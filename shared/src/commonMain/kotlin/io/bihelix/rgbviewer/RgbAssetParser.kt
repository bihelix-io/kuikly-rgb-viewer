package io.bihelix.rgbviewer

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlin.math.pow

private val rgbJson = Json {
    ignoreUnknownKeys = true
    isLenient = true
}

fun parseRgbAssets(payload: String): List<RgbAsset> {
    val root = rgbJson.parseToJsonElement(payload)
    val source = when (root) {
        is JsonObject -> root["data"] ?: root
        else -> root
    }

    val parsed = collectAssetObjects(source).mapNotNull { (fallbackTxid, obj) ->
        val contractId = obj.stringValue("contract_id", "asset_id", "rgb_asset_id")
        if (contractId.isBlank()) {
            null
        } else {
            RgbAsset(
                contractId = contractId,
                ticker = obj.stringValue("ticker", "asset_ticker", "tick").ifBlank { "RGB" },
                rawAmount = obj.doubleValue("rgb_amount", "amount", "balance"),
                address = obj.stringValue("address", "owner_address"),
                status = obj.stringValue("status").ifBlank { "Unknown" },
                decimals = obj.intValue("decimal", "decimals", "precision", "asset_precision"),
                txid = obj.stringValue("txid").ifBlank { fallbackTxid },
            )
        }
    }

    return mergeAssets(parsed)
}

private fun collectAssetObjects(element: JsonElement, txid: String? = null): List<Pair<String?, JsonObject>> {
    return when (element) {
        is JsonArray -> element.flatMap { collectAssetObjects(it, txid) }
        is JsonObject -> {
            if (element.containsAssetId()) {
                listOf(txid to element)
            } else {
                element.entries.flatMap { (key, value) ->
                    collectAssetObjects(value, txid = key.takeIf { it.isNotBlank() } ?: txid)
                }
            }
        }
        else -> emptyList()
    }
}

private fun JsonObject.containsAssetId(): Boolean {
    return containsKey("contract_id") || containsKey("asset_id") || containsKey("rgb_asset_id")
}

private fun mergeAssets(assets: List<RgbAsset>): List<RgbAsset> {
    val merged = linkedMapOf<String, RgbAsset>()

    assets.forEach { asset ->
        val current = merged[asset.contractId]
        if (current == null) {
            merged[asset.contractId] = asset
        } else {
            merged[asset.contractId] = current.copy(
                rawAmount = current.rawAmount + asset.rawAmount,
                status = preferredStatus(current.status, asset.status),
                txid = current.txid ?: asset.txid,
            )
        }
    }

    return merged.values.toList()
}

private fun preferredStatus(left: String, right: String): String {
    return when {
        left.equals("Confirmed", ignoreCase = true) -> left
        right.equals("Confirmed", ignoreCase = true) -> right
        right.isNotBlank() && left.isBlank() -> right
        else -> left
    }
}

private fun JsonObject.stringValue(vararg keys: String): String {
    return keys.firstNotNullOfOrNull { key ->
        val primitive = this[key] as? JsonPrimitive
        primitive?.contentOrNull?.takeIf { it.isNotBlank() }
    } ?: ""
}

private fun JsonObject.doubleValue(vararg keys: String): Double {
    return keys.firstNotNullOfOrNull { key ->
        val primitive = this[key]?.jsonPrimitive ?: return@firstNotNullOfOrNull null
        primitive.doubleOrNull ?: primitive.contentOrNull?.toDoubleOrNull()
    } ?: 0.0
}

private fun JsonObject.intValue(vararg keys: String): Int {
    return keys.firstNotNullOfOrNull { key ->
        val primitive = this[key]?.jsonPrimitive ?: return@firstNotNullOfOrNull null
        primitive.intOrNull ?: primitive.contentOrNull?.toIntOrNull()
    } ?: 0
}

fun formatRgbAmount(rawAmount: Double, decimals: Int): String {
    if (rawAmount == 0.0) {
        return "0"
    }
    if (decimals <= 0) {
        return rawAmount.toPlainText()
    }

    val value = rawAmount / 10.0.pow(decimals.coerceAtMost(18))
    return value.toPlainText().trimEnd('0').trimEnd('.').ifBlank { "0" }
}

private fun Double.toPlainText(): String {
    val text = toString()
    if (!text.contains('E') && !text.contains('e')) {
        return text
    }

    val parts = text.lowercase().split('e')
    val significand = parts.getOrNull(0)?.replace(".", "") ?: return text
    val decimalPlaces = parts.getOrNull(0)?.substringAfter('.', "")?.length ?: 0
    val exponent = parts.getOrNull(1)?.toIntOrNull() ?: return text
    val shift = exponent - decimalPlaces

    return if (shift >= 0) {
        significand + "0".repeat(shift)
    } else {
        val index = significand.length + shift
        if (index > 0) {
            significand.substring(0, index) + "." + significand.substring(index)
        } else {
            "0." + "0".repeat(-index) + significand
        }
    }
}
