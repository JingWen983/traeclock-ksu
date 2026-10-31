package cn.traeclock.manager

import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.graphics.drawable.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import cn.traeclock.manager.ui.effect.BgEffectBackground
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.window.WindowDialog

/** Repository / project links shown on the About page. */
private const val REPO_URL = "https://github.com/JingWen983/traeclock-ksu"
private const val ISSUES_URL = "$REPO_URL/issues"
private const val UPSTREAM_URL = "https://github.com/smart-open/TraeWorkAssistant"
private const val NAVBAR_URL = "https://github.com/HyperNavBar/HyperNavBar"
private const val GPL_URL = "https://www.gnu.org/licenses/gpl-3.0.txt"

/**
 * About page, modelled on HyperNavBar's `AboutPage` (AGPL/GPL-3.0 upstream):
 * a floating logo block (app icon + name + version) that fades out as the list
 * scrolls under it, followed by link cards and a credits dialog.
 *
 * Differences from upstream are deliberate and structural: no `textureBlur`
 * card backdrop (this app has no per-card screenshot layer — the glass pill
 * samples the page itself), and no "based on template" line.
 */
@Composable
fun AboutPage() {
    val ctx = LocalContext.current
    val uriHandler = LocalUriHandler.current
    val density = LocalDensity.current

    // Launcher icon straight from the package manager → always identical to the
    // home-screen tile (upstream does the same).
    val iconPx = with(density) { 96.dp.roundToPx() }
    val appIcon = remember(ctx, iconPx) {
        runCatching {
            ctx.packageManager.getApplicationIcon(ctx.applicationInfo)
                .toBitmap(iconPx, iconPx)
                .asImageBitmap()
        }.getOrNull()
    }

    val pkg = remember(ctx) {
        runCatching { ctx.packageManager.getPackageInfo(ctx.packageName, 0) }.getOrNull()
    }
    val versionName = pkg?.versionName ?: "—"
    @Suppress("DEPRECATION")
    val versionCode =
        if (android.os.Build.VERSION.SDK_INT >= 28) pkg?.longVersionCode ?: 0L
        else pkg?.versionCode?.toLong() ?: 0L

    // The module ships separately from the APK (same version for now, but they
    // can drift) — show what is actually installed in /data/adb/modules.
    var moduleVersion by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) { moduleVersion = RootCtl.moduleVersion() }
    }

    var showCredits by remember { mutableStateOf(false) }

    val listState = rememberLazyListState()
    var logoHeight by remember { mutableStateOf(0.dp) }
    // 0 = logo fully visible, 1 = list scrolled past it (logo gone)
    val progress by remember {
        derivedStateOf {
            if (listState.firstVisibleItemIndex > 0) {
                1f
            } else {
                (listState.firstVisibleItemScrollOffset / 420f).coerceIn(0f, 1f)
            }
        }
    }

    val topInset = LocalPageInsets.current.calculateTopPadding()
    val navInset = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()

    // HyperNavBar AboutPage 同款动态背景（RuntimeShader 渐变光斑，OS3 预设）：
    // 深浅色都保留渐变，但基底色跟随全 App 底色（深色 = 纯黑，光斑浮在黑底上；
    // 浅色 = surface 白底）。随滚动进度淡出（alpha = 1 - progress）。
    // 不支持 RuntimeShader（API < 33）时组件内部自动退化为纯色 Box。
    BgEffectBackground(
        dynamicBackground = true,
        isFullSize = true,
        modifier = Modifier.fillMaxSize(),
        alpha = { 1f - progress },
        // 基底跟随全 App 底色（深色纯黑 / 浅色 Miuix 背景）；未提供时用 Miuix surface
        baseColor = LocalAppBackground.current ?: MiuixTheme.colorScheme.surface,
    ) {
        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(
                start = 12.dp,
                end = 12.dp,
                top = topInset + 8.dp,
                // no glass bar on this sub-page: just clear the system nav area
                bottom = navInset + 28.dp,
            ),
        ) {
            // room for the floating logo block (top padding 8 + logo box 28 + gap 8)
            item { Spacer(Modifier.height(logoHeight + 44.dp)) }

            item {
                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column {
                        ArrowPreference(
                            title = "源代码",
                            summary = "github.com/JingWen983/traeclock-ksu",
                            onClick = { uriHandler.openUri(REPO_URL) },
                        )
                        ArrowPreference(
                            title = "问题反馈",
                            summary = "GitHub Issues（附机型 + 日志更容易定位）",
                            onClick = { uriHandler.openUri(ISSUES_URL) },
                        )
                    }
                }
            }

            item {
                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column {
                        ArrowPreference(
                            title = "开源许可",
                            summary = "GPL-3.0（整体）",
                            onClick = { uriHandler.openUri(GPL_URL) },
                        )
                        ArrowPreference(
                            title = "上游项目与致谢",
                            summary = "TraeWorkAssistant · HyperNavBar · 第三方组件",
                            onClick = { showCredits = true },
                        )
                    }
                }
            }

            item {
                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
                        InfoLine("管理器版本", "$versionName ($versionCode)")
                        InfoLine("模块版本", moduleVersion ?: "未检测到")
                        InfoLine("技术栈", "Miuix 0.9.4 · Compose · Haze · libsu")
                        InfoLine("数据位置", "/data/adb/traeclock（仅 root 可读）")
                    }
                }
            }

            item {
                Card(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        "全部数据保存在本机，不上传任何服务器。\n" +
                            "签到逻辑移植自 TraeWorkAssistant，液态玻璃底栏移植自 HyperNavBar。",
                        style = MiuixTheme.textStyles.footnote1,
                        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.fillMaxWidth().padding(16.dp),
                    )
                }
            }
        }

        // floating logo block (fades + drifts up as the list scrolls under it)
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(top = topInset + 28.dp)
                .alpha(1f - progress)
                .onSizeChanged { logoHeight = with(density) { it.height.toDp() } },
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(
                Modifier
                    .size(96.dp)
                    .clip(RoundedCornerShape(26.dp)),
                contentAlignment = Alignment.Center,
            ) {
                if (appIcon != null) {
                    Image(
                        bitmap = appIcon,
                        contentDescription = null,
                        modifier = Modifier.size(96.dp),
                    )
                } else {
                    Box(
                        Modifier
                            .fillMaxSize()
                            .clip(RoundedCornerShape(26.dp)),
                    )
                }
            }
            Text(
                text = "TraeClock",
                modifier = Modifier.padding(top = 12.dp, bottom = 2.dp),
                color = MiuixTheme.colorScheme.onBackground,
                fontWeight = FontWeight.Bold,
                fontSize = 32.sp,
            )
            Text(
                text = "版本 $versionName ($versionCode)",
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 14.sp,
            )
            Text(
                text = "签到守护 · KernelSU 模块",
                modifier = Modifier.padding(top = 2.dp),
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 12.sp,
            )
        }
    }

    WindowDialog(
        show = showCredits,
        title = "上游项目与致谢",
        onDismissRequest = { showCredits = false },
    ) {
        Column(
            Modifier.fillMaxWidth().padding(bottom = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            CreditLine(
                "TraeWorkAssistant",
                "MIT · 三应用签到 / 防风控 / 凭证链路的移植来源",
                UPSTREAM_URL,
                uriHandler,
            )
            CreditLine(
                "HyperNavBar",
                "GPL-3.0 · 液态玻璃底栏组件（SukiSU-Ultra 血统）",
                NAVBAR_URL,
                uriHandler,
            )
            Text(
                "第三方组件：Miuix（HyperOS Compose 组件）、Jetpack Compose、Haze（渐进模糊）、" +
                    "libsu（root shell）、Gson。",
                style = MiuixTheme.textStyles.footnote1,
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            )
            TextButton(
                text = "关闭",
                onClick = { showCredits = false },
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

/** Static label/value row used by the version card. */
@Composable
private fun InfoLine(label: String, value: String) {
    Column(Modifier.padding(vertical = 6.dp)) {
        Text(label, style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
        Text(value, style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.onSurface)
    }
}

/** A tappable credit line: project name + what we took from it. */
@Composable
private fun CreditLine(
    name: String,
    summary: String,
    url: String,
    uriHandler: androidx.compose.ui.platform.UriHandler,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .clickable { uriHandler.openUri(url) }
            .padding(horizontal = 4.dp, vertical = 6.dp),
    ) {
        Text(
            name,
            style = MiuixTheme.textStyles.body1,
            color = MiuixTheme.colorScheme.primary,
            modifier = Modifier.fillMaxWidth().padding(bottom = 2.dp),
        )
        Text(summary, style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
    }
}
