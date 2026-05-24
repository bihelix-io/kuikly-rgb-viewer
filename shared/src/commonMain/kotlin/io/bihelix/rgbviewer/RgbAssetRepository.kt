package io.bihelix.rgbviewer

import io.ktor.client.HttpClient
import io.ktor.client.request.get
import io.ktor.client.statement.bodyAsText
import io.ktor.http.encodeURLParameter

object RgbAssetRepository {
    const val DEFAULT_BASE_URL = "https://node-testnet.bihelix.io"

    suspend fun fetchLayer1Assets(baseUrl: String, address: String): Result<List<RgbAsset>> {
        val normalizedBaseUrl = baseUrl.trim().trimEnd('/').ifBlank { DEFAULT_BASE_URL }
        val normalizedAddress = address.trim()

        if (normalizedAddress.isBlank()) {
            return Result.failure(IllegalArgumentException("请输入地址"))
        }

        val client = HttpClient()
        return try {
            val payload = client
                .get("$normalizedBaseUrl/v3/asset?address=${normalizedAddress.encodeURLParameter()}")
                .bodyAsText()
            Result.success(parseRgbAssets(payload))
        } catch (error: Throwable) {
            Result.failure(error)
        } finally {
            client.close()
        }
    }
}
