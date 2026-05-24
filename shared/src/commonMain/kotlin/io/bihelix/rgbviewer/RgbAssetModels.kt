package io.bihelix.rgbviewer

data class RgbAsset(
    val contractId: String,
    val ticker: String,
    val rawAmount: Double,
    val address: String,
    val status: String,
    val decimals: Int,
    val txid: String?,
) {
    val displayAmount: String
        get() = formatRgbAmount(rawAmount, decimals)
}
