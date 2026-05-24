package io.bihelix.rgbviewer

import kotlin.test.Test
import kotlin.test.assertEquals

class RgbAssetParserTest {
    @Test
    fun mergesAssetsByContractId() {
        val assets = parseRgbAssets(
            """
            {
              "tx-a": [
                {
                  "contract_id": "rgb:asset-one",
                  "ticker": "RNA",
                  "rgb_amount": 100000000,
                  "address": "bc1p...",
                  "status": "Unconfirmed",
                  "decimal": 8
                }
              ],
              "tx-b": [
                {
                  "contract_id": "rgb:asset-one",
                  "ticker": "RNA",
                  "rgb_amount": 250000000,
                  "address": "bc1p...",
                  "status": "Confirmed",
                  "decimal": 8
                }
              ]
            }
            """.trimIndent()
        )

        assertEquals(1, assets.size)
        assertEquals("rgb:asset-one", assets.first().contractId)
        assertEquals(350000000.0, assets.first().rawAmount)
        assertEquals("3.5", assets.first().displayAmount)
        assertEquals("Confirmed", assets.first().status)
    }
}
