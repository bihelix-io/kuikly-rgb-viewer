use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bitcoin::{
    Address, CompressedPublicKey, Network,
    address::{AddressType, KnownHrp, NetworkUnchecked},
    secp256k1::Secp256k1,
    sign_message::{MessageSignature, signed_msg_hash},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    net::SocketAddr,
    path::{Component, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Mutex,
};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

const DEFAULT_UPSTREAM: &str = "https://node-testnet.bihelix.io";
const DEFAULT_MEMPOOL: &str = "https://mempool.space";
const DEFAULT_UTXO_UPSTREAM: &str = "https://node.bihelix.io/v3";
const DEFAULT_TOKEN_LIST_UPSTREAM: &str = "https://node.bihelix.io/v3";
const DEFAULT_TOKEN_IMAGE_BASE: &str = "https://static.bihelix.io/";

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    token_metadata: BTreeMap<String, Value>,
    token_list_error: Option<String>,
    redis_url: Option<String>,
    public_utxos: Arc<Mutex<BTreeSet<String>>>,
}

#[derive(Debug, Deserialize)]
struct AssetQuery {
    address: Option<String>,
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MempoolAddressQuery {
    address: Option<String>,
    network: Option<String>,
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UtxoAssetQuery {
    utxo: Option<String>,
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UtxoAccessQuery {
    utxo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PublicUtxoRequest {
    utxo: Option<String>,
    wallet: Option<String>,
    message: Option<String>,
    signature: Option<String>,
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WalletVerifyRequest {
    utxo: Option<String>,
    wallet: Option<String>,
    message: Option<String>,
    signature: Option<String>,
    provider: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RgbAssetView {
    contract_id: String,
    ticker: String,
    name: String,
    raw_amount: f64,
    display_amount: String,
    address: String,
    status: String,
    decimal: u32,
    logo_url: String,
    description: String,
    supply: Option<f64>,
    metadata: Option<Value>,
    txids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TokenMetadataView {
    contract_id: String,
    ticker: String,
    name: String,
    precision: u32,
    supply: Option<f64>,
    supply_display: String,
    logo_url: String,
    description: String,
    ext: Option<Value>,
    metadata: Value,
}

#[derive(Debug, Clone, Serialize)]
struct AddressTxView {
    txid: String,
    confirmed: bool,
    block_height: Option<u64>,
    block_time: Option<u64>,
    received: u64,
    sent: u64,
    net: i64,
    fee: u64,
    direction: String,
    inputs: usize,
    outputs: usize,
    input_utxos: Vec<AddressUtxoView>,
    output_utxos: Vec<AddressUtxoView>,
}

#[derive(Debug, Clone, Serialize)]
struct AddressUtxoView {
    kind: String,
    outpoint: String,
    txid: String,
    vout: u64,
    value: u64,
    address: String,
}

pub async fn serve(addr: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("create http client")?;
    let token_list = fetch_token_list(&client).await;
    let token_list_error = token_list.as_ref().err().map(|err| err.to_string());
    let token_metadata = token_list.unwrap_or_else(|err| {
        tracing::warn!(error = ?err, "token-list load failed");
        BTreeMap::new()
    });
    let redis_url = env::var("REDIS_URL").ok().filter(|item| !item.trim().is_empty());
    let public_utxos = Arc::new(Mutex::new(BTreeSet::new()));

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/rgb/assets", get(rgb_assets))
        .route("/api/mempool/address", get(mempool_address_txs))
        .route("/api/utxo/access", get(utxo_access))
        .route("/api/utxo/public", post(public_utxo))
        .route("/api/utxo/assets", get(utxo_assets))
        .route("/api/wallet/verify", post(wallet_verify))
        .route("/api/tokens", get(tokens_api))
        .route("/api/tokens/{contract_id}", get(token_api))
        .route("/tokens", get(tokens_page))
        .route("/token/{contract_id}", get(token_page))
        .fallback(get(web_asset))
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::very_permissive())
        .with_state(AppState {
            client,
            token_metadata,
            token_list_error,
            redis_url,
            public_utxos,
        });

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    let local_addr = listener
        .local_addr()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0)));
    println!("RGB asset viewer listening on http://{local_addr}");
    axum::serve(listener, app).await.context("serve http")
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "rgb-asset-viewer",
        "default_upstream": DEFAULT_UPSTREAM,
        "default_mempool": DEFAULT_MEMPOOL,
        "default_utxo_upstream": DEFAULT_UTXO_UPSTREAM,
        "default_token_list_upstream": DEFAULT_TOKEN_LIST_UPSTREAM,
        "default_token_image_base": DEFAULT_TOKEN_IMAGE_BASE,
        "redis_configured": state.redis_url.is_some(),
        "token_count": state.token_metadata.len(),
        "token_list_error": state.token_list_error,
    }))
}

async fn rgb_assets(State(state): State<AppState>, Query(query): Query<AssetQuery>) -> Response {
    match fetch_assets(&state, query).await {
        Ok(payload) => Json(payload).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": err.to_string(),
            })),
        )
            .into_response(),
    }
}

async fn mempool_address_txs(
    State(state): State<AppState>,
    Query(query): Query<MempoolAddressQuery>,
) -> Response {
    match fetch_mempool_address_txs(&state, query).await {
        Ok(payload) => Json(payload).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": err.to_string(),
            })),
        )
            .into_response(),
    }
}

async fn utxo_assets(
    State(state): State<AppState>,
    Query(query): Query<UtxoAssetQuery>,
) -> Response {
    match fetch_utxo_assets(&state, query).await {
        Ok(payload) => Json(payload).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": err.to_string(),
            })),
        )
            .into_response(),
    }
}

async fn utxo_access(
    State(state): State<AppState>,
    Query(query): Query<UtxoAccessQuery>,
) -> Response {
    let utxo = query.utxo.unwrap_or_default().trim().to_string();
    if utxo.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "missing utxo",
            })),
        )
            .into_response();
    }
    if !valid_outpoint(&utxo) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "utxo must be formatted as txid:vout",
            })),
        )
            .into_response();
    }

    let key = format!("redis/utxo/{utxo}");
    let keys = [key.clone()];
    if state.public_utxos.lock().await.contains(&utxo) {
        return Json(json!({
            "ok": true,
            "authorized": true,
            "checked": true,
            "public": true,
            "key": key,
            "keys": keys,
            "source": "memory",
        }))
        .into_response();
    }

    let Some(redis_url) = state.redis_url.as_deref() else {
        return Json(json!({
            "ok": true,
            "authorized": false,
            "checked": false,
            "keys": keys,
            "reason": "REDIS_URL not configured",
        }))
        .into_response();
    };

    match redis_any_exists(redis_url, &keys).await {
        Ok(Some(key)) => Json(json!({
            "ok": true,
            "authorized": true,
            "checked": true,
            "key": key,
            "keys": keys,
        }))
        .into_response(),
        Ok(None) => Json(json!({
            "ok": true,
            "authorized": false,
            "checked": true,
            "keys": keys,
            "reason": "utxo authorization key not found",
        }))
        .into_response(),
        Err(err) => Json(json!({
            "ok": true,
            "authorized": false,
            "checked": false,
            "keys": keys,
            "reason": err.to_string(),
        }))
        .into_response(),
    }
}

async fn public_utxo(
    State(state): State<AppState>,
    Json(payload): Json<PublicUtxoRequest>,
) -> Response {
    let utxo = payload.utxo.unwrap_or_default().trim().to_string();
    if utxo.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "missing utxo",
            })),
        )
            .into_response();
    }
    if !valid_outpoint(&utxo) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "utxo must be formatted as txid:vout",
            })),
        )
            .into_response();
    }

    let wallet = payload.wallet.unwrap_or_default().trim().to_string();
    let message = payload.message.unwrap_or_default();
    let signature = payload.signature.unwrap_or_default();
    let provider = payload.provider.unwrap_or_default();
    if let Err(err) = verify_wallet_signature(&utxo, &wallet, &message, &signature) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "ok": false,
                "error": err.to_string(),
            })),
        )
            .into_response();
    }

    let key = format!("redis/utxo/{utxo}");
    let published_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|item| item.as_secs())
        .unwrap_or_default();
    let value = json!({
        "utxo": utxo,
        "wallet": wallet,
        "provider": provider,
        "message": message,
        "signature": signature,
        "published_at": published_at,
        "source": "kuikly-rgb-viewer",
    })
    .to_string();

    if let Some(redis_url) = state.redis_url.as_deref() {
        match redis_set_value(redis_url, &key, &value).await {
            Ok(()) => {
                state.public_utxos.lock().await.insert(utxo.clone());
                Json(json!({
                    "ok": true,
                    "public": true,
                    "persisted": true,
                    "key": key,
                }))
                .into_response()
            }
            Err(err) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "ok": false,
                    "error": err.to_string(),
                    "key": key,
                })),
            )
                .into_response(),
        }
    } else {
        state.public_utxos.lock().await.insert(utxo);
        Json(json!({
            "ok": true,
            "public": true,
            "persisted": false,
            "key": key,
            "reason": "REDIS_URL not configured; stored in this server session",
        }))
        .into_response()
    }
}

async fn wallet_verify(Json(payload): Json<WalletVerifyRequest>) -> Response {
    let utxo = payload.utxo.unwrap_or_default().trim().to_string();
    let wallet = payload.wallet.unwrap_or_default().trim().to_string();
    let message = payload.message.unwrap_or_default();
    let signature = payload.signature.unwrap_or_default();
    let provider = payload.provider.unwrap_or_default();

    if let Err(err) = validate_outpoint_field(&utxo) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "verified": false,
                "error": err.to_string(),
            })),
        )
            .into_response();
    }

    match verify_wallet_signature(&utxo, &wallet, &message, &signature) {
        Ok(()) => Json(json!({
            "ok": true,
            "verified": true,
            "utxo": utxo,
            "wallet": wallet,
            "provider": provider,
        }))
        .into_response(),
        Err(err) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "ok": false,
                "verified": false,
                "error": err.to_string(),
            })),
        )
            .into_response(),
    }
}

async fn tokens_api(State(state): State<AppState>) -> Json<Value> {
    let tokens = token_views(&state);
    Json(json!({
        "ok": true,
        "count": tokens.len(),
        "tokens": tokens,
        "token_list_error": state.token_list_error,
    }))
}

async fn token_api(
    State(state): State<AppState>,
    AxumPath(contract_id): AxumPath<String>,
) -> Response {
    match token_view(&state, &contract_id) {
        Some(token) => Json(json!({
            "ok": true,
            "token": token,
            "token_list_error": state.token_list_error,
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("token not found: {contract_id}"),
                "token_count": state.token_metadata.len(),
                "token_list_error": state.token_list_error,
            })),
        )
            .into_response(),
    }
}

async fn tokens_page(State(state): State<AppState>) -> Response {
    Html(render_tokens_page(&state)).into_response()
}

async fn token_page(
    State(state): State<AppState>,
    AxumPath(contract_id): AxumPath<String>,
) -> Response {
    match token_view(&state, &contract_id) {
        Some(token) => Html(render_token_page(&token)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Html(render_token_not_found_page(&state, &contract_id)),
        )
            .into_response(),
    }
}

async fn fetch_utxo_assets(state: &AppState, query: UtxoAssetQuery) -> Result<Value> {
    let utxo = query.utxo.unwrap_or_default().trim().to_string();
    if utxo.is_empty() {
        return Err(anyhow!("missing utxo"));
    }
    if !valid_outpoint(&utxo) {
        return Err(anyhow!("utxo must be formatted as txid:vout"));
    }

    let base_url = query
        .base_url
        .unwrap_or_else(|| DEFAULT_UTXO_UPSTREAM.to_string())
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(anyhow!("base_url must start with http:// or https://"));
    }

    let upstream = format!("{base_url}/utxoasset?utxo={}", urlencoding::encode(&utxo));
    let response = state
        .client
        .get(&upstream)
        .send()
        .await
        .with_context(|| format!("request utxo upstream {upstream}"))?;
    let status = response.status();
    let body = response.text().await.context("read utxo body")?;

    if !status.is_success() {
        return Err(anyhow!("utxo upstream returned {status}: {body}"));
    }

    let raw: Value = serde_json::from_str(&body).context("utxo upstream did not return json")?;
    let mut assets = normalize_assets(&raw);
    enrich_assets(&mut assets, &state.token_metadata);

    Ok(json!({
        "ok": true,
        "utxo": utxo,
        "upstream": upstream,
        "count": assets.len(),
        "assets": assets,
        "raw": raw,
        "token_count": state.token_metadata.len(),
        "token_list_error": state.token_list_error,
    }))
}

async fn fetch_mempool_address_txs(state: &AppState, query: MempoolAddressQuery) -> Result<Value> {
    let address = query.address.unwrap_or_default().trim().to_string();
    if address.is_empty() {
        return Err(anyhow!("missing address"));
    }

    let network = query
        .network
        .unwrap_or_else(|| "mainnet".to_string())
        .trim()
        .to_lowercase();
    let base_url = query
        .base_url
        .unwrap_or_else(|| mempool_base_url(&network))
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(anyhow!("base_url must start with http:// or https://"));
    }

    let upstream = format!("{base_url}/api/address/{address}/txs");
    let response = state
        .client
        .get(&upstream)
        .send()
        .await
        .with_context(|| format!("request mempool upstream {upstream}"))?;
    let status = response.status();
    let body = response.text().await.context("read mempool body")?;

    if !status.is_success() {
        return Err(anyhow!("mempool returned {status}: {body}"));
    }

    let raw: Value = serde_json::from_str(&body).context("mempool did not return json")?;
    let mut txs = normalize_mempool_txs(&address, &raw);
    txs.sort_by(|left, right| tx_sort_key(right).cmp(&tx_sort_key(left)));

    let received_total: u64 = txs.iter().map(|tx| tx.received).sum();
    let sent_total: u64 = txs.iter().map(|tx| tx.sent).sum();
    let unconfirmed = txs.iter().filter(|tx| !tx.confirmed).count();

    Ok(json!({
        "ok": true,
        "network": network,
        "upstream": upstream,
        "count": txs.len(),
        "received_total": received_total,
        "sent_total": sent_total,
        "unconfirmed": unconfirmed,
        "txs": txs,
    }))
}

fn mempool_base_url(network: &str) -> String {
    match network {
        "testnet" => format!("{DEFAULT_MEMPOOL}/testnet"),
        "testnet4" => format!("{DEFAULT_MEMPOOL}/testnet4"),
        "signet" => format!("{DEFAULT_MEMPOOL}/signet"),
        _ => DEFAULT_MEMPOOL.to_string(),
    }
}

fn valid_outpoint(value: &str) -> bool {
    let Some((txid, vout)) = value.split_once(':') else {
        return false;
    };

    txid.len() == 64
        && txid.chars().all(|item| item.is_ascii_hexdigit())
        && !vout.is_empty()
        && vout.parse::<u64>().is_ok()
}

fn validate_outpoint_field(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(anyhow!("missing utxo"));
    }
    if !valid_outpoint(value) {
        return Err(anyhow!("utxo must be formatted as txid:vout"));
    }
    Ok(())
}

fn wallet_auth_message(wallet: &str, utxo: &str) -> String {
    format!(
        "BiHelix RGB Viewer\nAction: view UTXO RGB assets\nAddress: {wallet}\nUTXO: {utxo}"
    )
}

fn verify_wallet_signature(utxo: &str, wallet: &str, message: &str, signature: &str) -> Result<()> {
    validate_outpoint_field(utxo)?;
    if wallet.is_empty() {
        return Err(anyhow!("missing wallet address"));
    }
    if signature.trim().is_empty() {
        return Err(anyhow!("missing wallet signature"));
    }

    let expected = wallet_auth_message(wallet, utxo);
    if message != expected {
        return Err(anyhow!("wallet message does not match this UTXO and address"));
    }

    let signature_bytes = BASE64_STANDARD
        .decode(signature.trim())
        .map_err(|_| anyhow!("wallet signature must be standard base64"))?;
    let message_signature = MessageSignature::from_slice(&signature_bytes)
        .map_err(|err| anyhow!("invalid wallet signature: {err}"))?;
    let address = wallet
        .parse::<Address<NetworkUnchecked>>()
        .with_context(|| format!("invalid wallet address: {wallet}"))?;
    let msg_hash = signed_msg_hash(message);
    let secp = Secp256k1::verification_only();
    let pubkey = message_signature
        .recover_pubkey(&secp, msg_hash)
        .map_err(|err| anyhow!("signature cannot recover public key: {err}"))?;
    let compressed_pubkey = CompressedPublicKey::try_from(pubkey)
        .map_err(|_| anyhow!("wallet signature must use a compressed public key"))?;

    let networks = [
        Network::Bitcoin,
        Network::Testnet,
        Network::Testnet4,
        Network::Signet,
        Network::Regtest,
    ];
    for network in networks {
        if !address.is_valid_for_network(network) {
            continue;
        }
        let checked = match address.clone().require_network(network) {
            Ok(item) => item,
            Err(_) => continue,
        };
        let candidates = [
            Address::p2pkh(pubkey.pubkey_hash(), network).to_string(),
            Address::p2shwpkh(&compressed_pubkey, network).to_string(),
            Address::p2wpkh(&compressed_pubkey, KnownHrp::from(network)).to_string(),
            Address::p2tr(
                &secp,
                compressed_pubkey.into(),
                None,
                KnownHrp::from(network),
            )
            .to_string(),
        ];
        if candidates.iter().any(|item| item == &checked.to_string()) {
            return Ok(());
        }
        if checked.address_type() == Some(AddressType::P2pkh) {
            let signed_by = message_signature
                .is_signed_by_address(&secp, &checked, msg_hash)
                .unwrap_or(false);
            if signed_by {
                return Ok(());
            }
        }
    }

    Err(anyhow!("signature does not match wallet address"))
}

#[derive(Debug)]
struct RedisEndpoint {
    host: String,
    port: u16,
    password: Option<String>,
    db: Option<u32>,
}

async fn redis_any_exists(redis_url: &str, keys: &[String]) -> Result<Option<String>> {
    let endpoint = parse_redis_url(redis_url)?;
    let mut stream = redis_connect(&endpoint).await?;

    for key in keys {
        redis_send_command(&mut stream, &["EXISTS", key]).await?;
        if redis_read_integer(&mut stream).await? > 0 {
            return Ok(Some(key.clone()));
        }
    }

    Ok(None)
}

async fn redis_set_value(redis_url: &str, key: &str, value: &str) -> Result<()> {
    let endpoint = parse_redis_url(redis_url)?;
    let mut stream = redis_connect(&endpoint).await?;
    redis_send_command(&mut stream, &["SET", key, value]).await?;
    redis_expect_ok(&mut stream).await.context("redis SET failed")
}

async fn redis_connect(endpoint: &RedisEndpoint) -> Result<TcpStream> {
    let mut stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .with_context(|| format!("connect redis {}:{}", endpoint.host, endpoint.port))?;

    if let Some(password) = endpoint.password.as_deref() {
        redis_send_command(&mut stream, &["AUTH", password]).await?;
        redis_expect_ok(&mut stream).await.context("redis AUTH failed")?;
    }

    if let Some(db) = endpoint.db {
        let db = db.to_string();
        redis_send_command(&mut stream, &["SELECT", &db]).await?;
        redis_expect_ok(&mut stream).await.context("redis SELECT failed")?;
    }

    Ok(stream)
}

fn parse_redis_url(redis_url: &str) -> Result<RedisEndpoint> {
    let rest = redis_url
        .trim()
        .strip_prefix("redis://")
        .ok_or_else(|| anyhow!("REDIS_URL must start with redis://"))?;
    let (authority, db) = rest.split_once('/').unwrap_or((rest, ""));
    let (auth, host_port) = authority
        .rsplit_once('@')
        .map(|(auth, host_port)| (Some(auth), host_port))
        .unwrap_or((None, authority));
    let password = auth.and_then(|item| item.strip_prefix(':').or(Some(item))).and_then(|item| {
        if item.is_empty() {
            None
        } else {
            Some(item.to_string())
        }
    });
    let (host, port) = host_port
        .rsplit_once(':')
        .map(|(host, port)| (host.to_string(), port.parse::<u16>()))
        .map(|(host, port)| port.map(|port| (host, port)))
        .transpose()
        .context("invalid redis port")?
        .unwrap_or_else(|| (host_port.to_string(), 6379));
    if host.is_empty() {
        return Err(anyhow!("REDIS_URL host is empty"));
    }
    let db = db
        .split_once('?')
        .map(|(db, _)| db)
        .unwrap_or(db)
        .trim();
    let db = if db.is_empty() {
        None
    } else {
        Some(db.parse::<u32>().context("invalid redis db")?)
    };

    Ok(RedisEndpoint {
        host,
        port,
        password,
        db,
    })
}

async fn redis_send_command(stream: &mut TcpStream, parts: &[&str]) -> Result<()> {
    let mut command = format!("*{}\r\n", parts.len()).into_bytes();
    for part in parts {
        command.extend_from_slice(format!("${}\r\n", part.len()).as_bytes());
        command.extend_from_slice(part.as_bytes());
        command.extend_from_slice(b"\r\n");
    }
    stream.write_all(&command).await.context("write redis command")
}

async fn redis_expect_ok(stream: &mut TcpStream) -> Result<()> {
    let line = redis_read_line(stream).await?;
    if line.starts_with("+OK") {
        Ok(())
    } else {
        Err(anyhow!("unexpected redis response: {line}"))
    }
}

async fn redis_read_integer(stream: &mut TcpStream) -> Result<i64> {
    let line = redis_read_line(stream).await?;
    let value = line
        .strip_prefix(':')
        .ok_or_else(|| anyhow!("unexpected redis response: {line}"))?;
    value.parse::<i64>().context("parse redis integer")
}

async fn redis_read_line(stream: &mut TcpStream) -> Result<String> {
    let mut buffer = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let read = stream.read(&mut byte).await.context("read redis response")?;
        if read == 0 {
            return Err(anyhow!("redis closed connection"));
        }
        buffer.push(byte[0]);
        if buffer.ends_with(b"\r\n") {
            buffer.truncate(buffer.len().saturating_sub(2));
            return String::from_utf8(buffer).context("redis response was not utf-8");
        }
        if buffer.len() > 4096 {
            return Err(anyhow!("redis response line too long"));
        }
    }
}

fn tx_sort_key(tx: &AddressTxView) -> (u8, u64) {
    if tx.confirmed {
        (1, tx.block_time.unwrap_or(0))
    } else {
        (2, u64::MAX)
    }
}

fn normalize_mempool_txs(address: &str, raw: &Value) -> Vec<AddressTxView> {
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|tx| {
            let object = tx.as_object()?;
            let txid = string_value(object, &["txid"]);
            if txid.is_empty() {
                return None;
            }

            let input_utxos = object
                .get("vin")
                .and_then(Value::as_array)
                .map(|inputs| {
                    inputs
                        .iter()
                        .filter_map(Value::as_object)
                        .filter_map(|input| {
                            let prevout = input.get("prevout").and_then(Value::as_object)?;
                            let prev_address = string_value(prevout, &["scriptpubkey_address"]);
                            if prev_address != address {
                                return None;
                            }

                            let prev_txid = string_value(input, &["txid"]);
                            if prev_txid.is_empty() {
                                return None;
                            }
                            let prev_vout = u64_value(input, &["vout"]);

                            Some(AddressUtxoView {
                                kind: "input".to_string(),
                                outpoint: format!("{prev_txid}:{prev_vout}"),
                                txid: prev_txid,
                                vout: prev_vout,
                                value: u64_value(prevout, &["value"]),
                                address: prev_address,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let output_utxos = object
                .get("vout")
                .and_then(Value::as_array)
                .map(|outputs| {
                    outputs
                        .iter()
                        .enumerate()
                        .filter_map(|(index, output)| {
                            let output = output.as_object()?;
                            let output_address = string_value(output, &["scriptpubkey_address"]);
                            if output_address != address {
                                return None;
                            }

                            Some(AddressUtxoView {
                                kind: "output".to_string(),
                                outpoint: format!("{txid}:{index}"),
                                txid: txid.clone(),
                                vout: index as u64,
                                value: u64_value(output, &["value"]),
                                address: output_address,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let received = output_utxos.iter().map(|utxo| utxo.value).sum();
            let sent = input_utxos.iter().map(|utxo| utxo.value).sum();

            let status = object.get("status").and_then(Value::as_object);
            let confirmed = status
                .and_then(|status| status.get("confirmed"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let block_height = status
                .and_then(|status| status.get("block_height"))
                .and_then(Value::as_u64);
            let block_time = status
                .and_then(|status| status.get("block_time"))
                .and_then(Value::as_u64);
            let inputs = object
                .get("vin")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            let outputs = object
                .get("vout")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            let net = received as i64 - sent as i64;
            let direction = if net > 0 {
                "receive"
            } else if net < 0 {
                "send"
            } else {
                "self"
            };

            Some(AddressTxView {
                txid,
                confirmed,
                block_height,
                block_time,
                received,
                sent,
                net,
                fee: u64_value(object, &["fee"]),
                direction: direction.to_string(),
                inputs,
                outputs,
                input_utxos,
                output_utxos,
            })
        })
        .collect()
}

async fn fetch_assets(state: &AppState, query: AssetQuery) -> Result<Value> {
    let address = query.address.unwrap_or_default().trim().to_string();
    if address.is_empty() {
        return Err(anyhow!("missing address"));
    }

    let base_url = query
        .base_url
        .unwrap_or_else(|| DEFAULT_UPSTREAM.to_string())
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(anyhow!("base_url must start with http:// or https://"));
    }

    let upstream = format!(
        "{base_url}/v3/asset?address={}",
        urlencoding::encode(&address)
    );
    let response = state
        .client
        .get(&upstream)
        .send()
        .await
        .with_context(|| format!("request upstream {upstream}"))?;
    let status = response.status();
    let body = response.text().await.context("read upstream body")?;

    if !status.is_success() {
        return Err(anyhow!("upstream returned {status}: {body}"));
    }

    let raw: Value = serde_json::from_str(&body).context("upstream did not return json")?;
    let mut assets = normalize_assets(&raw);
    enrich_assets(&mut assets, &state.token_metadata);

    Ok(json!({
        "ok": true,
        "upstream": upstream,
        "count": assets.len(),
        "assets": assets,
        "raw": raw,
        "token_count": state.token_metadata.len(),
        "token_list_error": state.token_list_error,
    }))
}

fn normalize_assets(raw: &Value) -> Vec<RgbAssetView> {
    let source = raw.get("data").unwrap_or(raw);
    if let Some(assets) = normalize_contract_map_assets(source) {
        return assets;
    }

    let mut found = Vec::new();
    collect_asset_objects(source, None, &mut found);

    let mut merged: BTreeMap<String, RgbAssetView> = BTreeMap::new();
    for (txid, object) in found {
        let contract_id = string_value(object, &["contract_id", "asset_id", "rgb_asset_id"]);
        if contract_id.is_empty() {
            continue;
        }

        let decimal = u32_value(
            object,
            &["decimal", "decimals", "precision", "asset_precision"],
        );
        let raw_amount = f64_value(object, &["rgb_amount", "amount", "balance"]);
        let txid = string_value(object, &["txid"]).or(txid);

        merged
            .entry(contract_id.clone())
            .and_modify(|asset| {
                asset.raw_amount += raw_amount;
                asset.display_amount = format_rgb_amount(asset.raw_amount, asset.decimal);
                asset.status = prefer_status(&asset.status, &string_value(object, &["status"]));
                if !txid.is_empty() && !asset.txids.iter().any(|item| item == &txid) {
                    asset.txids.push(txid.clone());
                }
            })
            .or_insert_with(|| RgbAssetView {
                contract_id,
                ticker: string_value(object, &["ticker", "asset_ticker", "tick"]).or("RGB".into()),
                name: string_value(object, &["name", "asset_name"]),
                raw_amount,
                display_amount: format_rgb_amount(raw_amount, decimal),
                address: string_value(object, &["address", "owner_address"]),
                status: string_value(object, &["status"]).or("Unknown".into()),
                decimal,
                logo_url: String::new(),
                description: String::new(),
                supply: None,
                metadata: None,
                txids: if txid.is_empty() { vec![] } else { vec![txid] },
            });
    }

    merged.into_values().collect()
}

fn normalize_contract_map_assets(source: &Value) -> Option<Vec<RgbAssetView>> {
    let object = source.as_object()?;
    if !object.keys().any(|key| key.starts_with("rgb:")) {
        return None;
    }

    let mut assets = Vec::new();
    for (contract_id, value) in object {
        if !contract_id.starts_with("rgb:") {
            continue;
        }

        let entries = value.as_array().cloned().unwrap_or_default();
        let raw_amount = entries
            .iter()
            .filter_map(Value::as_object)
            .map(|entry| f64_value(entry, &["rgb_amount", "amount", "balance"]))
            .sum::<f64>();
        let txids = entries
            .iter()
            .filter_map(Value::as_object)
            .map(|entry| string_value(entry, &["from", "txid"]))
            .filter(|from| !from.is_empty())
            .fold(Vec::new(), |mut acc, from| {
                if !acc.iter().any(|item| item == &from) {
                    acc.push(from);
                }
                acc
            });

        assets.push(RgbAssetView {
            contract_id: contract_id.clone(),
            ticker: short_contract_label(contract_id),
            name: String::new(),
            raw_amount,
            display_amount: trim_number(raw_amount),
            address: String::new(),
            status: "Found".to_string(),
            decimal: 0,
            logo_url: String::new(),
            description: String::new(),
            supply: None,
            metadata: None,
            txids,
        });
    }

    Some(assets)
}

async fn fetch_token_list(client: &reqwest::Client) -> Result<BTreeMap<String, Value>> {
    let base_url = std::env::var("TOKEN_LIST_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_TOKEN_LIST_UPSTREAM.to_string())
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(anyhow!(
            "token_base_url must start with http:// or https://"
        ));
    }

    let upstream = format!("{base_url}/asset/list");
    let response = client
        .get(&upstream)
        .send()
        .await
        .with_context(|| format!("request token list upstream {upstream}"))?;
    let status = response.status();
    let body = response.text().await.context("read token list body")?;

    if !status.is_success() {
        return Err(anyhow!("token list returned {status}: {body}"));
    }

    let raw: Value = serde_json::from_str(&body).context("token list did not return json")?;
    let mut found = BTreeMap::new();
    collect_token_metadata(&raw, &mut found);
    Ok(found)
}

fn collect_token_metadata(value: &Value, out: &mut BTreeMap<String, Value>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_token_metadata(item, out);
            }
        }
        Value::Object(object) => {
            let contract_id = string_value(object, &["contract_id", "asset_id", "rgb_asset_id"]);
            if contract_id.starts_with("rgb:") {
                out.insert(contract_id, value.clone());
            } else {
                for child in object.values() {
                    collect_token_metadata(child, out);
                }
            }
        }
        _ => {}
    }
}

fn enrich_assets(assets: &mut [RgbAssetView], metadata: &BTreeMap<String, Value>) {
    for asset in assets {
        let Some(token) = metadata.get(&asset.contract_id).and_then(Value::as_object) else {
            continue;
        };

        let ext = token.get("ext").and_then(Value::as_object);
        let ticker = string_value(token, &["ticker"]);
        let name = string_value(token, &["name", "asset_name"]).or(ext
            .map(|item| string_value(item, &["asset_name"]))
            .unwrap_or_default());
        let precision = u32_value(token, &["precision", "decimal", "decimals"]);
        let supply = f64_value(token, &["supply", "amount", "total_supply"]);
        let logo = ext
            .map(|item| string_value(item, &["logo", "image", "icon"]))
            .unwrap_or_default();
        let description = ext
            .map(|item| string_value(item, &["description"]))
            .unwrap_or_default();

        if !ticker.is_empty() {
            asset.ticker = ticker;
        }
        if !name.is_empty() {
            asset.name = name;
        }
        if precision > 0 {
            asset.decimal = precision;
            asset.display_amount = format_rgb_amount(asset.raw_amount, precision);
        }
        if supply > 0.0 {
            asset.supply = Some(supply);
        }
        if !logo.is_empty() {
            asset.logo_url = resolve_token_image(&logo);
        }
        if !description.is_empty() {
            asset.description = description;
        }
        asset.metadata = Some(Value::Object(token.clone()));
    }
}

fn resolve_token_image(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("{DEFAULT_TOKEN_IMAGE_BASE}{value}")
    }
}

fn token_views(state: &AppState) -> Vec<TokenMetadataView> {
    state
        .token_metadata
        .iter()
        .filter_map(|(contract_id, token)| token_view_from_value(contract_id, token))
        .collect()
}

fn token_view(state: &AppState, contract_id: &str) -> Option<TokenMetadataView> {
    state
        .token_metadata
        .get(contract_id)
        .and_then(|token| token_view_from_value(contract_id, token))
}

fn token_view_from_value(contract_id: &str, token: &Value) -> Option<TokenMetadataView> {
    let object = token.as_object()?;
    let ext = object.get("ext").cloned();
    let ext_object = ext.as_ref().and_then(Value::as_object);
    let ticker = string_value(object, &["ticker"]).or("RGB".to_string());
    let name = string_value(object, &["name", "asset_name"]).or(ext_object
        .map(|item| string_value(item, &["asset_name"]))
        .unwrap_or_default());
    let precision = u32_value(object, &["precision", "decimal", "decimals"]);
    let supply = f64_value(object, &["supply", "amount", "total_supply"]);
    let logo = ext_object
        .map(|item| string_value(item, &["logo", "image", "icon"]))
        .unwrap_or_default();
    let description = ext_object
        .map(|item| string_value(item, &["description"]))
        .unwrap_or_default();

    Some(TokenMetadataView {
        contract_id: contract_id.to_string(),
        ticker,
        name,
        precision,
        supply: if supply > 0.0 { Some(supply) } else { None },
        supply_display: if supply > 0.0 {
            format_rgb_amount(supply, precision)
        } else {
            "0".to_string()
        },
        logo_url: if logo.is_empty() {
            String::new()
        } else {
            resolve_token_image(&logo)
        },
        description,
        ext,
        metadata: token.clone(),
    })
}

fn render_tokens_page(state: &AppState) -> String {
    let cards = token_views(state)
        .into_iter()
        .map(|token| {
            let name = if token.name.is_empty() {
                token.contract_id.clone()
            } else {
                token.name.clone()
            };
            format!(
                r#"<a class="token-card" href="{href}">
                    {logo}
                    <div class="token-body">
                      <div class="token-symbol">{ticker}</div>
                      <div class="token-name">{name}</div>
                      <div class="token-meta">precision {precision} · supply {supply}</div>
                    </div>
                  </a>"#,
                href = html_escape(&token_page_href(&token.contract_id)),
                logo = render_token_logo(&token.logo_url, &token.ticker),
                ticker = html_escape(&token.ticker),
                name = html_escape(&name),
                precision = token.precision,
                supply = html_escape(&token.supply_display),
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let empty = if cards.is_empty() {
        format!(
            r#"<div class="empty">启动时没有加载到 token-list。{}</div>"#,
            state
                .token_list_error
                .as_ref()
                .map(|err| html_escape(err))
                .unwrap_or_default()
        )
    } else {
        cards
    };

    render_token_shell(
        "RGB Token List",
        &format!(
            r#"<main class="page">
                <nav><a href="/">地址记录</a></nav>
                <section class="hero-list">
                  <div>
                    <div class="eyebrow">token-list</div>
                    <h1>RGB 资产目录</h1>
                    <p>server 启动时拉取并缓存，共 {count} 个 token。</p>
                  </div>
                </section>
                <section class="token-grid">{empty}</section>
              </main>"#,
            count = state.token_metadata.len(),
            empty = empty,
        ),
    )
}

fn render_token_page(token: &TokenMetadataView) -> String {
    let display_name = if token.name.is_empty() {
        token.ticker.clone()
    } else {
        token.name.clone()
    };
    let description = if token.description.is_empty() {
        "暂无描述。".to_string()
    } else {
        token.description.clone()
    };

    render_token_shell(
        &format!("{} · {}", token.ticker, display_name),
        &format!(
            r#"<main class="page">
                <nav><a href="/">地址记录</a><a href="/tokens">Token List</a></nav>
                <section class="hero-token">
                  {logo}
                  <div>
                    <div class="eyebrow">RGB token</div>
                    <h1>{ticker}</h1>
                    <p class="token-title">{name}</p>
                    <p>{description}</p>
                  </div>
                </section>
                <section class="detail-grid">
                  <div class="detail-item"><span>Supply</span><strong>{supply}</strong></div>
                  <div class="detail-item"><span>Precision</span><strong>{precision}</strong></div>
                  <div class="detail-item wide"><span>Contract ID</span><strong>{contract}</strong></div>
                </section>
                <section class="json-panel">
                  <div class="panel-title">合约详情</div>
                  <pre>{metadata}</pre>
                </section>
              </main>"#,
            logo = render_token_logo(&token.logo_url, &token.ticker),
            ticker = html_escape(&token.ticker),
            name = html_escape(&display_name),
            description = html_escape(&description),
            supply = html_escape(&token.supply_display),
            precision = token.precision,
            contract = html_escape(&token.contract_id),
            metadata =
                html_escape(&serde_json::to_string_pretty(&token.metadata).unwrap_or_default()),
        ),
    )
}

fn render_token_not_found_page(state: &AppState, contract_id: &str) -> String {
    render_token_shell(
        "Token not found",
        &format!(
            r#"<main class="page">
                <nav><a href="/">地址记录</a><a href="/tokens">Token List</a></nav>
                <section class="json-panel">
                  <div class="panel-title">未找到 token</div>
                  <p>没有在启动时缓存的 token-list 中找到：</p>
                  <pre>{contract_id}</pre>
                  <p>当前缓存 {count} 个 token。{error}</p>
                </section>
              </main>"#,
            contract_id = html_escape(contract_id),
            count = state.token_metadata.len(),
            error = state
                .token_list_error
                .as_ref()
                .map(|err| html_escape(err))
                .unwrap_or_default(),
        ),
    )
}

fn render_token_shell(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{title}</title>
    <style>
      :root {{ color-scheme: light; --bg:#f5f7fb; --panel:#fff; --line:#dbe3f0; --text:#111827; --muted:#667085; --blue:#276ef1; --green:#147a47; }}
      * {{ box-sizing:border-box; }}
      body {{ margin:0; min-height:100vh; background:var(--bg); color:var(--text); font-family:Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
      a {{ color:var(--blue); text-decoration:none; }}
      .page {{ max-width:1100px; margin:0 auto; padding:28px 18px 46px; }}
      nav {{ display:flex; gap:14px; margin-bottom:22px; font-size:14px; }}
      .hero-list, .hero-token {{ border:1px solid var(--line); border-radius:8px; background:var(--panel); padding:24px; margin-bottom:18px; }}
      .hero-token {{ display:grid; grid-template-columns:96px minmax(0,1fr); gap:18px; align-items:center; }}
      .eyebrow {{ color:var(--green); font-size:12px; font-weight:850; text-transform:uppercase; letter-spacing:0; }}
      h1 {{ margin:4px 0; font-size:34px; line-height:40px; overflow-wrap:anywhere; }}
      p {{ margin:6px 0 0; color:var(--muted); line-height:22px; }}
      .token-title {{ color:#344054; font-weight:800; }}
      .token-logo {{ width:74px; height:74px; border-radius:999px; border:1px solid var(--line); background:#f8fafc; object-fit:cover; display:grid; place-items:center; color:#344054; font-weight:900; }}
      .token-grid {{ display:grid; grid-template-columns:repeat(3, minmax(0, 1fr)); gap:12px; }}
      .token-card {{ display:grid; grid-template-columns:54px minmax(0,1fr); gap:12px; align-items:center; min-height:92px; border:1px solid var(--line); border-radius:8px; background:var(--panel); padding:13px; color:inherit; }}
      .token-card .token-logo {{ width:48px; height:48px; font-size:12px; }}
      .token-symbol {{ font-weight:900; font-size:16px; }}
      .token-name, .token-meta {{ color:var(--muted); font-size:12px; overflow-wrap:anywhere; }}
      .detail-grid {{ display:grid; grid-template-columns:repeat(3, minmax(0, 1fr)); gap:12px; margin-bottom:18px; }}
      .detail-item, .json-panel, .empty {{ border:1px solid var(--line); border-radius:8px; background:var(--panel); padding:15px; }}
      .detail-item span {{ display:block; color:var(--muted); font-size:12px; margin-bottom:6px; }}
      .detail-item strong {{ font-size:20px; overflow-wrap:anywhere; }}
      .detail-item.wide {{ grid-column:1 / -1; }}
      .panel-title {{ font-weight:900; margin-bottom:10px; }}
      pre {{ margin:0; white-space:pre-wrap; overflow-wrap:anywhere; color:#1d2939; font-size:12px; line-height:18px; }}
      @media (max-width: 760px) {{ .hero-token, .token-grid, .detail-grid {{ grid-template-columns:1fr; }} h1 {{ font-size:28px; line-height:34px; }} }}
    </style>
  </head>
  <body>{body}</body>
</html>"#,
        title = html_escape(title),
        body = body,
    )
}

fn render_token_logo(logo_url: &str, ticker: &str) -> String {
    if logo_url.is_empty() {
        format!(
            r#"<div class="token-logo">{}</div>"#,
            html_escape(&ticker.chars().take(4).collect::<String>())
        )
    } else {
        format!(
            r#"<img class="token-logo" src="{}" alt="{}" />"#,
            html_escape(logo_url),
            html_escape(ticker)
        )
    }
}

fn token_page_href(contract_id: &str) -> String {
    format!("/token/{}", urlencoding::encode(contract_id))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

fn short_contract_label(contract_id: &str) -> String {
    contract_id
        .strip_prefix("rgb:")
        .and_then(|value| value.split('-').next())
        .filter(|value| !value.is_empty())
        .map(|value| format!("RGB {value}"))
        .unwrap_or_else(|| "RGB".to_string())
}

fn collect_asset_objects<'a>(
    value: &'a Value,
    txid: Option<String>,
    out: &mut Vec<(String, &'a serde_json::Map<String, Value>)>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_asset_objects(item, txid.clone(), out);
            }
        }
        Value::Object(object) => {
            if object.contains_key("contract_id")
                || object.contains_key("asset_id")
                || object.contains_key("rgb_asset_id")
            {
                out.push((txid.unwrap_or_default(), object));
            } else {
                for (key, child) in object {
                    collect_asset_objects(child, Some(key.clone()), out);
                }
            }
        }
        _ => {}
    }
}

fn string_value(object: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| match value {
            Value::String(text) => Some(text.trim().to_string()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn f64_value(object: &serde_json::Map<String, Value>, keys: &[&str]) -> f64 {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.parse::<f64>().ok(),
            _ => None,
        })
        .unwrap_or(0.0)
}

fn u32_value(object: &serde_json::Map<String, Value>, keys: &[&str]) -> u32 {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| match value {
            Value::Number(number) => number.as_u64().map(|item| item as u32),
            Value::String(text) => text.parse::<u32>().ok(),
            _ => None,
        })
        .unwrap_or(0)
}

fn u64_value(object: &serde_json::Map<String, Value>, keys: &[&str]) -> u64 {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| match value {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse::<u64>().ok(),
            _ => None,
        })
        .unwrap_or(0)
}

fn prefer_status(current: &str, next: &str) -> String {
    if current.eq_ignore_ascii_case("confirmed") || next.is_empty() {
        current.to_string()
    } else if next.eq_ignore_ascii_case("confirmed") || current.is_empty() {
        next.to_string()
    } else {
        current.to_string()
    }
}

fn format_rgb_amount(raw_amount: f64, decimal: u32) -> String {
    if raw_amount == 0.0 {
        return "0".to_string();
    }
    if decimal == 0 {
        return trim_number(raw_amount);
    }

    trim_number(raw_amount / 10_f64.powi(decimal.min(18) as i32))
}

fn trim_number(value: f64) -> String {
    let text = format!("{value:.12}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

async fn web_asset(uri: axum::http::Uri) -> Response {
    match read_web_asset(uri.path()).await {
        Ok((content_type, bytes)) => {
            let mut response = bytes.into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            response
        }
        Err(err) => {
            tracing::warn!(error = ?err, path = uri.path(), "web asset not found");
            (StatusCode::NOT_FOUND, "not found").into_response()
        }
    }
}

async fn read_web_asset(request_path: &str) -> Result<(&'static str, Vec<u8>)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static");
    let relative = safe_relative_path(request_path).unwrap_or_else(|| PathBuf::from("index.html"));
    let mut path = root.join(relative);
    if path.is_dir() {
        path = path.join("index.html");
    }
    if !path.exists() {
        path = root.join("index.html");
    }

    let content_type = content_type_for(&path);
    let bytes = tokio::fs::read(&path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    Ok((content_type, bytes))
}

fn safe_relative_path(request_path: &str) -> Option<PathBuf> {
    let request_path = request_path.trim_start_matches('/');
    if request_path.is_empty() {
        return None;
    }

    let mut path = PathBuf::new();
    for component in PathBuf::from(request_path).components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => return None,
        }
    }
    Some(path)
}

fn content_type_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|item| item.to_str())
        .unwrap_or("")
    {
        "html" => "text/html;charset=utf-8",
        "css" => "text/css;charset=utf-8",
        "js" => "application/javascript;charset=utf-8",
        "json" => "application/json;charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

trait OrString {
    fn or(self, fallback: String) -> String;
}

impl OrString for String {
    fn or(self, fallback: String) -> String {
        if self.is_empty() { fallback } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_rgb_contract_map_assets() {
        let raw = json!({
            "rgb:nykNCHhT-BgKdtCi-ilF89kf-JilBhg0-JfInd9k-7MyyYOE": [
                {
                    "from": "4a78ec34193e08e849cafd9f03f2e343f24ad906760151dc5cf71070de61b6cf",
                    "rgb_amount": 200000000
                }
            ],
            "rgb:v4QTAynM-graJmCZ-x_L93hs-QY~NcQg-Cqrj_lS-Mminq6c": [
                {
                    "from": "a4d6493fe597abe6ea4714cccb1a39525d281ab22d6df74db29b849a997e78ff",
                    "rgb_amount": 10000000000u64
                }
            ]
        });

        let assets = normalize_assets(&raw);

        assert_eq!(assets.len(), 2);
        assert_eq!(
            assets[0].contract_id,
            "rgb:nykNCHhT-BgKdtCi-ilF89kf-JilBhg0-JfInd9k-7MyyYOE"
        );
        assert_eq!(assets[0].raw_amount, 200000000.0);
        assert_eq!(
            assets[0].txids,
            vec!["4a78ec34193e08e849cafd9f03f2e343f24ad906760151dc5cf71070de61b6cf"]
        );
    }
}
