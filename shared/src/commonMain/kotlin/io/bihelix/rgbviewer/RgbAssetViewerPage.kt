package io.bihelix.rgbviewer

import com.tencent.kuikly.core.annotations.Page
import com.tencent.kuikly.core.base.Border
import com.tencent.kuikly.core.base.BorderStyle
import com.tencent.kuikly.core.base.Color
import com.tencent.kuikly.core.base.ComposeEvent
import com.tencent.kuikly.core.base.ViewBuilder
import com.tencent.kuikly.core.pager.Pager
import com.tencent.kuikly.core.reactive.handler.observable
import com.tencent.kuikly.core.views.Input
import com.tencent.kuikly.core.views.Text
import com.tencent.kuikly.core.views.View
import com.tencent.kuikly.core.views.List as KuiklyList
import com.tencent.kuikly.core.views.compose.Button
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.launch

@Page("RgbAssetViewerPage")
internal class RgbAssetViewerPage : Pager() {
    private val scope = MainScope()

    private var baseUrl by observable(RgbAssetRepository.DEFAULT_BASE_URL)
    private var address by observable("")
    private var statusText by observable("输入地址后点击查询")
    private var loading by observable(false)
    private var assets by observable(emptyList<RgbAsset>())

    override fun createEvent(): ComposeEvent {
        return ComposeEvent()
    }

    override fun body(): ViewBuilder {
        val ctx = this
        return {
            attr {
                backgroundColor(Color(0xFFF7F8FA))
                flexDirectionColumn()
                padding(16f)
                autoDarkEnable(false)
            }

            Text {
                attr {
                    text("RGB 资产查看器")
                    color(Color(0xFF14171F))
                    fontSize(24f)
                    fontWeightBold()
                    lineHeight(32f)
                }
            }

            Text {
                attr {
                    text("按地址查询 Layer1 RGB 资产余额")
                    color(Color(0xFF5E6675))
                    fontSize(14f)
                    lineHeight(22f)
                    marginTop(4f)
                }
            }

            FieldLabel("服务地址")
            Input {
                attr {
                    text(ctx.baseUrl)
                    placeholder("https://node-testnet.bihelix.io")
                    color(Color(0xFF14171F))
                    fontSize(14f)
                    height(46f)
                    borderRadius(8f)
                    paddingLeft(12f)
                    paddingRight(12f)
                    backgroundColor(Color.WHITE)
                    border(Border(1f, BorderStyle.SOLID, Color(0xFFD9DEE8)))
                }
                event {
                    textDidChange {
                        ctx.baseUrl = it.text
                    }
                }
            }

            FieldLabel("地址")
            Input {
                attr {
                    text(ctx.address)
                    placeholder("输入 BTC/RGB 地址")
                    color(Color(0xFF14171F))
                    fontSize(14f)
                    height(46f)
                    borderRadius(8f)
                    paddingLeft(12f)
                    paddingRight(12f)
                    backgroundColor(Color.WHITE)
                    border(Border(1f, BorderStyle.SOLID, Color(0xFFD9DEE8)))
                }
                event {
                    textDidChange {
                        ctx.address = it.text
                    }
                }
            }

            Button {
                attr {
                    titleAttr {
                        text(if (ctx.loading) "查询中..." else "查询资产")
                        color(Color.WHITE)
                        fontSize(15f)
                        fontWeightBold()
                    }
                    marginTop(16f)
                    height(46f)
                    borderRadius(8f)
                    backgroundColor(if (ctx.loading) Color(0xFF8A93A5) else Color(0xFF276EF1))
                }
                event {
                    click {
                        ctx.loadAssets()
                    }
                }
            }

            Text {
                attr {
                    text(ctx.statusText)
                    color(if (ctx.statusText.startsWith("查询失败")) Color(0xFFD92D20) else Color(0xFF4B5565))
                    fontSize(13f)
                    lineHeight(20f)
                    marginTop(12f)
                }
            }

            SummaryBar(ctx.assets)

            KuiklyList {
                attr {
                    flex(1f)
                    marginTop(12f)
                }
                ctx.assets.forEach { asset ->
                    AssetCard(asset)
                }
            }
        }
    }

    private fun loadAssets() {
        if (loading) {
            return
        }

        loading = true
        statusText = "正在查询 ${address.trim().ifBlank { "当前地址" }}"

        scope.launch {
            RgbAssetRepository.fetchLayer1Assets(baseUrl, address)
                .onSuccess { result ->
                    assets = result
                    statusText = if (result.isEmpty()) {
                        "未找到该地址的 RGB 资产"
                    } else {
                        "已加载 ${result.size} 个 RGB 资产"
                    }
                }
                .onFailure { error ->
                    assets = emptyList()
                    statusText = "查询失败：${error.message ?: "请检查服务地址和网络"}"
                }
            loading = false
        }
    }
}

private fun com.tencent.kuikly.core.base.ViewContainer<*, *>.FieldLabel(label: String) {
    Text {
        attr {
            text(label)
            color(Color(0xFF252B37))
            fontSize(13f)
            fontWeightMedium()
            lineHeight(18f)
            marginTop(16f)
            marginBottom(8f)
        }
    }
}

private fun com.tencent.kuikly.core.base.ViewContainer<*, *>.SummaryBar(
    assets: kotlin.collections.List<RgbAsset>,
) {
    if (assets.isEmpty()) {
        return
    }

    View {
        attr {
            marginTop(14f)
            padding(12f)
            borderRadius(8f)
            backgroundColor(Color(0xFFEAF5F1))
            border(Border(1f, BorderStyle.SOLID, Color(0xFFB9DEC8)))
            flexDirectionRow()
            justifyContentSpaceBetween()
            alignItemsCenter()
        }
        Text {
            attr {
                text("资产数")
                color(Color(0xFF285B47))
                fontSize(13f)
                lineHeight(18f)
            }
        }
        Text {
            attr {
                text(assets.size.toString())
                color(Color(0xFF123D2D))
                fontSize(18f)
                fontWeightBold()
                lineHeight(22f)
            }
        }
    }
}

private fun com.tencent.kuikly.core.base.ViewContainer<*, *>.AssetCard(asset: RgbAsset) {
    View {
        attr {
            marginBottom(10f)
            padding(14f)
            borderRadius(8f)
            backgroundColor(Color.WHITE)
            border(Border(1f, BorderStyle.SOLID, Color(0xFFE3E7EF)))
            flexDirectionColumn()
        }

        View {
            attr {
                flexDirectionRow()
                justifyContentSpaceBetween()
                alignItemsCenter()
            }
            Text {
                attr {
                    text(asset.ticker)
                    color(Color(0xFF111827))
                    fontSize(18f)
                    fontWeightBold()
                    lineHeight(24f)
                }
            }
            Text {
                attr {
                    text(asset.status)
                    color(if (asset.status.equals("Confirmed", ignoreCase = true)) Color(0xFF157F3B) else Color(0xFFB54708))
                    fontSize(12f)
                    lineHeight(18f)
                }
            }
        }

        Text {
            attr {
                text(asset.displayAmount)
                color(Color(0xFF276EF1))
                fontSize(22f)
                fontWeightBold()
                lineHeight(30f)
                marginTop(8f)
            }
        }

        AssetMeta("Contract", asset.contractId)
        if (!asset.txid.isNullOrBlank()) {
            AssetMeta("Txid", asset.txid)
        }
        if (asset.address.isNotBlank()) {
            AssetMeta("Address", asset.address)
        }
    }
}

private fun com.tencent.kuikly.core.base.ViewContainer<*, *>.AssetMeta(label: String, value: String) {
    Text {
        attr {
            text("$label: $value")
            color(Color(0xFF667085))
            fontSize(12f)
            lineHeight(18f)
            lines(2)
            textOverFlowMiddle()
            marginTop(6f)
        }
    }
}
