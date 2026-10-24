package cn.traeclock.manager

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.shape.CircleShape
import com.kyant.backdrop.backdrops.layerBackdrop
import com.kyant.backdrop.backdrops.rememberLayerBackdrop
import com.kyant.backdrop.drawBackdrop
import com.kyant.backdrop.effects.blur
import com.kyant.backdrop.effects.lens
import com.kyant.backdrop.effects.vibrancy
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.FloatingActionButton
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.LinearProgressIndicator
import top.yukonga.miuix.kmp.basic.SmallTitle
import top.yukonga.miuix.kmp.basic.Switch
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.window.WindowBottomSheet
import top.yukonga.miuix.kmp.preference.WindowDropdownPreference
import top.yukonga.miuix.kmp.preference.SwitchPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.viewinterop.AndroidView

class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        RootCtlAppContextHolder.context = applicationContext
        // Edge-to-edge: draw behind the system bars so the top app bar's Miuix
        // theme color extends under the (transparent) status bar — the status
        // bar then "auto-tints" with the page background in light/dark modes.
        enableEdgeToEdge(
            statusBarStyle = androidx.activity.SystemBarStyle.auto(
                android.graphics.Color.TRANSPARENT,
                android.graphics.Color.TRANSPARENT,
            ),
            navigationBarStyle = androidx.activity.SystemBarStyle.auto(
                android.graphics.Color.TRANSPARENT,
                android.graphics.Color.TRANSPARENT,
            ),
        )
        setContent { TraeClockApp() }
    }
}

@Composable
fun TraeClockApp() {
    val controller = remember { top.yukonga.miuix.kmp.theme.ThemeController(top.yukonga.miuix.kmp.theme.ColorSchemeMode.System) }
    top.yukonga.miuix.kmp.theme.MiuixTheme(controller = controller) {
        var tab by remember { mutableIntStateOf(0) }
        val tabs = listOf("主页", "账号", "日志", "设置")
        // liquid-glass bottom bar: the page content is captured into a backdrop
        // layer and refracted through the bar (Kyant0/AndroidLiquidGlass)
        // (theme colors are @Composable reads — hoist them OUT of the DrawScope
        // lambdas, which are plain non-composable DrawScope.() -> Unit)
        val backdropBg = MiuixTheme.colorScheme.background
        val backdrop = rememberLayerBackdrop {
            drawRect(backdropBg)
            drawContent()
        }
        Box(Modifier.fillMaxSize().background(backdropBg)) {
            Box(
                Modifier
                    .fillMaxSize()
                    .layerBackdrop(backdrop)
                    .statusBarsPadding()
            ) {
                when (tab) {
                    0 -> HomePage()
                    1 -> AccountsPage()
                    2 -> LogPage()
                    3 -> SettingsPage()
                }
            }
            GlassBottomBar(
                tabs = tabs,
                selected = tab,
                onSelect = { tab = it },
                backdrop = backdrop,
                Modifier.align(Alignment.BottomCenter),
            )
        }
    }
}

/** Floating liquid-glass pill bottom bar (SukiSU-style): pages scroll under it,
 *  the backdrop refracts whatever is behind the bar. */
@Composable
private fun GlassBottomBar(
    tabs: List<String>,
    selected: Int,
    onSelect: (Int) -> Unit,
    backdrop: com.kyant.backdrop.Backdrop,
    modifier: Modifier = Modifier,
) {
    // hoisted @Composable theme reads (DrawScope lambdas are non-composable)
    val surfaceTint = MiuixTheme.colorScheme.surface.copy(alpha = 0.5f)
    Box(
        modifier
            .padding(horizontal = 24f.dp)
            .navigationBarsPadding()
            .padding(bottom = 12f.dp)
            .fillMaxWidth()
            .height(64f.dp)
            .drawBackdrop(
                backdrop = backdrop,
                shape = { CircleShape },
                effects = {
                    vibrancy()
                    blur(4f.dp.toPx())
                    lens(16f.dp.toPx(), 32f.dp.toPx())
                },
                onDrawSurface = { drawRect(surfaceTint) },
            ),
        contentAlignment = Alignment.Center,
    ) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 8.dp),
            horizontalArrangement = Arrangement.SpaceEvenly,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            tabs.forEachIndexed { index, label ->
                val sel = index == selected
                Column(
                    Modifier
                        .clickable(
                            interactionSource = null,
                            indication = null,
                        ) { onSelect(index) }
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Icon(
                        imageVector = MiuixTabIcons.forIndex(index),
                        contentDescription = label,
                        tint = if (sel) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    )
                    Text(
                        label,
                        style = MiuixTheme.textStyles.footnote2,
                        color = if (sel) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
        }
    }
}

/** Tab icons: tiny self-drawn vectors (no icons artifact dependency). */
private object MiuixTabIcons {
    private fun glyph(name: String, block: androidx.compose.ui.graphics.vector.PathBuilder.() -> Unit): androidx.compose.ui.graphics.vector.ImageVector =
        androidx.compose.ui.graphics.vector.ImageVector.Builder(
            name = name,
            defaultWidth = 26.dp,
            defaultHeight = 26.dp,
            viewportWidth = 24f,
            viewportHeight = 24f,
        ).apply { path(fill = SolidColor(Color.Black)) { block() } }.build()

    private val home by lazy {
        glyph("TabHome") {
            moveTo(12f, 3f); lineTo(2f, 12f); lineTo(5f, 12f); lineTo(5f, 20f); lineTo(11f, 20f)
            lineTo(11f, 14f); lineTo(13f, 14f); lineTo(13f, 20f); lineTo(19f, 20f); lineTo(19f, 12f)
            lineTo(22f, 12f); close()
        }
    }
    private val people by lazy {
        glyph("TabPeople") {
            moveTo(4f, 6f); lineTo(20f, 6f); lineTo(20f, 8f); lineTo(4f, 8f); close()
            moveTo(4f, 11f); lineTo(20f, 11f); lineTo(20f, 13f); lineTo(4f, 13f); close()
            moveTo(4f, 16f); lineTo(14f, 16f); lineTo(14f, 18f); lineTo(4f, 18f); close()
        }
    }
    private val person by lazy {
        glyph("TabPerson") {
            moveTo(11f, 5f); lineTo(13f, 5f); lineTo(13f, 11f); lineTo(19f, 11f); lineTo(19f, 13f)
            lineTo(13f, 13f); lineTo(13f, 19f); lineTo(11f, 19f); lineTo(11f, 13f); lineTo(5f, 13f)
            lineTo(5f, 11f); lineTo(11f, 11f); close()
        }
    }
    private val plus by lazy {
        glyph("TabAdd") {
            moveTo(11f, 4f); lineTo(13f, 4f); lineTo(13f, 11f); lineTo(20f, 11f); lineTo(20f, 13f)
            lineTo(13f, 13f); lineTo(13f, 20f); lineTo(11f, 20f); lineTo(11f, 13f); lineTo(4f, 13f)
            lineTo(4f, 11f); lineTo(11f, 11f); close()
        }
    }
    private val doc by lazy {
        glyph("TabDoc") {
            moveTo(6f, 2f); lineTo(15f, 2f); lineTo(20f, 7f); lineTo(20f, 22f); lineTo(6f, 22f); close()
            moveTo(14f, 3.5f); lineTo(14f, 8f); lineTo(18.5f, 8f); close()
        }
    }
    private val settings by lazy {
        glyph("TabSettings") {
            moveTo(12f, 8f)
            curveTo(14.2f, 8f, 16f, 9.8f, 16f, 12f)
            curveTo(16f, 14.2f, 14.2f, 16f, 12f, 16f)
            curveTo(9.8f, 16f, 8f, 14.2f, 8f, 12f)
            curveTo(8f, 9.8f, 9.8f, 8f, 12f, 8f)
            close()
            moveTo(10.5f, 2f); lineTo(13.5f, 2f); lineTo(14f, 5f); lineTo(16.5f, 6f); lineTo(19f, 4.5f)
            lineTo(21f, 6.5f); lineTo(19.5f, 9f); lineTo(20.5f, 11.5f); lineTo(23.5f, 12f); lineTo(23.5f, 12f)
            lineTo(20.5f, 12.5f); lineTo(19.5f, 15f); lineTo(21f, 17.5f); lineTo(19f, 19.5f); lineTo(16.5f, 18f)
            lineTo(14f, 19f); lineTo(13.5f, 22f); lineTo(10.5f, 22f); lineTo(10f, 19f); lineTo(7.5f, 18f)
            lineTo(5f, 19.5f); lineTo(3f, 17.5f); lineTo(4.5f, 15f); lineTo(3.5f, 12.5f); lineTo(0.5f, 12f)
            lineTo(3.5f, 11.5f); lineTo(4.5f, 9f); lineTo(3f, 6.5f); lineTo(5f, 4.5f); lineTo(7.5f, 6f)
            lineTo(10f, 5f); close()
        }
    }

    val forIndex: (Int) -> androidx.compose.ui.graphics.vector.ImageVector = { index ->
        when (index) {
            0 -> home; 1 -> people; 2 -> doc; else -> settings
        }
    }
    val add: androidx.compose.ui.graphics.vector.ImageVector get() = plus
}

// ── shared page scaffold (status gate) ───────────────────────────────────────

@Composable
fun StatusGate(content: @Composable (refresh: () -> Unit) -> Unit) {
    // RootCtl caches root/module probes process-wide; reuse them synchronously so
    // every tab switch renders instantly instead of re-forking su shells.
    val rootOk = RootCtl.isRootAvailable()
    val moduleOk = rootOk && RootCtl.isModuleInstalled()

    when {
        !rootOk -> GateCard("未获得 Root", "本应用需要 KernelSU/Magisk root 才能与模块通信。")
        !moduleOk -> GateCard("未检测到 TraeClock 模块", "请先在 KernelSU 管理器中刷入 TraeClock 模块 zip 并重启。")
        else -> content { }
    }
}

@Composable
private fun GateCard(title: String, summary: String) {
    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp)) {
                Text(title, style = MiuixTheme.textStyles.title2)
                Spacer(Modifier.height(6.dp))
                Text(summary, style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
            }
        }
    }
}

// ── 主页（守护进程状态 + 实时积分，合并自旧概览/看板页）──────────────────────

@Composable
fun HomePage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var status by remember { mutableStateOf(DataCache.status) }
        var snapshot by remember { mutableStateOf(DataCache.credits) }
        var diffs by remember { mutableStateOf<List<CreditDiffRow>>(emptyList()) }
        var busy by remember { mutableStateOf(false) }
        var msg by remember { mutableStateOf("") }
        var refreshing by remember { mutableStateOf(false) }
        var expanded by remember { mutableStateOf(setOf<String>()) }

        fun refreshStatus() {
            scope.launch {
                val s = withContext(Dispatchers.IO) { RootCtl.status()?.let { parseStatus(it) } }
                if (s != null) { DataCache.status = s; status = s }
            }
        }
        fun refreshCredits() {
            scope.launch {
                refreshing = true
                val s = withContext(Dispatchers.IO) { RootCtl.credits()?.let { parseCredits(it) } }
                if (s != null) { DataCache.credits = s; snapshot = s }
                refreshing = false
            }
        }
        fun refreshDiffs() {
            scope.launch {
                val d = withContext(Dispatchers.IO) { RootCtl.creditDiffs().map { parseCreditDiff(it) } }
                diffs = d
            }
        }
        LaunchedEffect(Unit) { refreshStatus(); refreshCredits(); refreshDiffs() }
        // real-time dashboard: auto refresh every 60s while page stays open
        LaunchedEffect(Unit) {
            while (true) {
                kotlinx.coroutines.delay(60_000)
                if (!refreshing) {
                    val s = withContext(Dispatchers.IO) { RootCtl.credits()?.let { parseCredits(it) } }
                    if (s != null) { DataCache.credits = s; snapshot = s }
                }
            }
        }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 12.dp, bottom = 96.dp)) {
            item {
                val s = status
                if (s != null) {
                    SmallTitle(text = "守护进程")
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                Text("状态", style = MiuixTheme.textStyles.body1)
                                Text(
                                    if (s.daemonPid != null) "运行中 (pid ${s.daemonPid})" else "未运行（重启设备后自启）",
                                    style = MiuixTheme.textStyles.body2,
                                    color = if (s.daemonPid != null) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                )
                            }
                            s.nextApp?.let { app ->
                                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                    Text("下次签到", style = MiuixTheme.textStyles.body1)
                                    Text("${Apps.displayName(app)} · ${s.nextInSecs?.let { formatDuration(it) } ?: "—"}", style = MiuixTheme.textStyles.body2)
                                }
                            }
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                Text("签到窗口", style = MiuixTheme.textStyles.body1)
                                Text(
                                    "%02d:%02d – %02d:%02d%s".format(
                                        s.windowStart / 60, s.windowStart % 60, s.windowEnd / 60, s.windowEnd % 60,
                                        if (s.jitter) "（随机抖动）" else "",
                                    ),
                                    style = MiuixTheme.textStyles.body2,
                                )
                            }
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                Text("账号间隔 / 指纹", style = MiuixTheme.textStyles.body1)
                                Text("${s.accountGap}s · ${s.platformFlavor}", style = MiuixTheme.textStyles.body2)
                            }
                        }
                    }
                    SmallTitle(text = "账号分组")
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Column {
                            Apps.ALL.forEach { app ->
                                val c = s.counts[app] ?: CountRow(0, 0, 0)
                                Row(
                                    Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
                                    horizontalArrangement = Arrangement.SpaceBetween,
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    Text(Apps.displayName(app), style = MiuixTheme.textStyles.body1)
                                    Text(
                                        "${c.enabled}/${c.total} 启用" + if (c.needsRelogin > 0) " · ${c.needsRelogin} 需重登" else "",
                                        style = MiuixTheme.textStyles.body2,
                                        color = if (c.needsRelogin > 0) MiuixTheme.colorScheme.error else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                    )
                                }
                            }
                        }
                    }
                    Spacer(Modifier.height(12.dp))
                    if (msg.isNotEmpty()) {
                        Text(msg, style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.primary)
                        Spacer(Modifier.height(8.dp))
                    }
                    TextButton(
                        text = if (busy) "签到中…" else "立即签到（全部）",
                        enabled = !busy,
                        onClick = {
                            busy = true; msg = "签到中…"
                            scope.launch {
                                val r = withContext(Dispatchers.IO) { RootCtl.runNow(null) }
                                busy = false
                                msg = if (r.code == 0) "已触发全部签到，结果见日志页" else "触发失败: ${r.stdout.take(120)}"
                                refreshStatus()
                                // reload the credit-change records after the round settles
                                refreshDiffs()
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    )
                } else {
                    Text("加载中…", style = MiuixTheme.textStyles.body2)
                }
            }

            // ── 积分变动（签到前后对比） ──
            item {
                SmallTitle(text = "积分变动")
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (diffs.isEmpty()) {
                            Text(
                                "暂无记录。每轮签到前后各读取一次积分并对比，变动会显示在这里。",
                                style = MiuixTheme.textStyles.body2,
                                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                            )
                        } else {
                            val today = java.text.SimpleDateFormat("yyyy-MM-dd", java.util.Locale.getDefault()).format(java.util.Date())
                            val todayDiffs = diffs.filter { it.date == today }
                            val todaySum = todayDiffs.mapNotNull { it.delta }.sum()
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.Bottom) {
                                Column {
                                    Text("今日合计", style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                                    Text(
                                        (if (todaySum >= 0) "+" else "") + todaySum.toString(),
                                        style = MiuixTheme.textStyles.title2,
                                        fontSize = 28.sp,
                                        fontWeight = FontWeight.Bold,
                                        color = if (todaySum >= 0) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.error,
                                    )
                                }
                                Text(
                                    "${todayDiffs.size} 条今日记录 · 近 90 天滚动",
                                    style = MiuixTheme.textStyles.footnote2,
                                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                    modifier = Modifier.align(Alignment.Bottom),
                                )
                            }
                            Spacer(Modifier.height(4.dp))
                            diffs.take(20).forEach { d ->
                                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                    Column(Modifier.weight(1f)) {
                                        Text(
                                            "${Apps.displayName(d.app)} · ${d.name.ifEmpty { d.userId }}",
                                            style = MiuixTheme.textStyles.body2,
                                        )
                                        Text(
                                            d.date + if (d.time.isNotEmpty()) " ${d.time}" else "",
                                            style = MiuixTheme.textStyles.footnote2,
                                            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                        )
                                    }
                                    Column(horizontalAlignment = Alignment.End) {
                                        Text(
                                            d.delta?.let { (if (it >= 0) "+" else "") + it } ?: "—",
                                            style = MiuixTheme.textStyles.body1,
                                            fontWeight = FontWeight.Bold,
                                            color = when {
                                                d.delta == null -> MiuixTheme.colorScheme.onSurfaceVariantSummary
                                                d.delta >= 0 -> MiuixTheme.colorScheme.primary
                                                else -> MiuixTheme.colorScheme.error
                                            },
                                        )
                                        if (d.before != null || d.after != null) {
                                            Text(
                                                "${d.before ?: "—"} → ${d.after ?: "—"}",
                                                style = MiuixTheme.textStyles.footnote2,
                                                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Spacer(Modifier.height(4.dp))
            }

            // ── 实时积分 ──
            item {
                val snap = snapshot
                if (snap == null) {
                    SmallTitle(text = "积分")
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        LinearProgressIndicator(modifier = Modifier.weight(1f))
                        Text("查询中…", style = MiuixTheme.textStyles.body2)
                    }
                } else {
                    SmallTitle(text = "实时积分")
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(16.dp)) {
                            Text("%.2f".format(snap.total), style = MiuixTheme.textStyles.title1, fontSize = 40.sp, fontWeight = FontWeight.Bold)
                            Spacer(Modifier.height(4.dp))
                            Text(
                                "更新于 ${snap.fetchedAt.take(19).replace('T', ' ')} · 每 60 秒自动刷新",
                                style = MiuixTheme.textStyles.footnote1,
                                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                            )
                        }
                    }
                    Spacer(Modifier.height(4.dp))
                    if (snap.rows.isEmpty()) {
                        Spacer(Modifier.height(12.dp))
                        Text("暂无账号，请到「登录」页添加", style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                    }
                    snap.rows.forEach { r ->
                        SmallTitle(text = "${Apps.displayName(r.app)} · ${r.name.ifEmpty { r.userId }}")
                        Card(modifier = Modifier.fillMaxWidth()) {
                            Column(Modifier.padding(16.dp)) {
                                if (!r.ok) {
                                    Text("查询失败：${r.message}", style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.error)
                                } else {
                                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.Bottom) {
                                        Column {
                                            Text(
                                                "%.2f".format(r.balance ?: 0.0),
                                                style = MiuixTheme.textStyles.title2,
                                                fontSize = 30.sp,
                                                fontWeight = FontWeight.Bold,
                                            )
                                            Text(
                                                listOfNotNull(
                                                    r.general?.let { "通用 ${"%.2f".format(it)}" },
                                                    r.work?.let { "${if (r.app == Apps.TRAE) "Work" else "资源包"} ${"%.2f".format(it)}" },
                                                    r.used?.let { "已用 ${"%.2f".format(it)}" },
                                                ).joinToString(" · "),
                                                style = MiuixTheme.textStyles.footnote1,
                                                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                            )
                                        }
                                        Column(horizontalAlignment = Alignment.End) {
                                            Text("最早到期", style = MiuixTheme.textStyles.footnote2, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                                            Text(fmtExpire(r.earliestExpire), style = MiuixTheme.textStyles.body2)
                                        }
                                    }
                                    if (r.packs.isNotEmpty()) {
                                        Spacer(Modifier.height(8.dp))
                                        val key = "${r.app}:${r.userId}"
                                        TextButton(
                                            text = if (key in expanded) "收起明细" else "包明细（${r.packs.size}）",
                                            onClick = {
                                                expanded = if (key in expanded) expanded - key else expanded + key
                                            },
                                        )
                                        AnimatedVisibility(visible = key in expanded) {
                                            Column(Modifier.padding(top = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                                r.packs.forEach { p ->
                                                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                                                        Column {
                                                            Text("${p.kind} · ${p.source}", style = MiuixTheme.textStyles.body2)
                                                            Text(
                                                                "到期 " + (p.expireText ?: fmtExpire(p.expireTime)),
                                                                style = MiuixTheme.textStyles.footnote2,
                                                                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                                            )
                                                        }
                                                        Text(
                                                            p.remaining?.let { "%.2f".format(it) } ?: "—",
                                                            style = MiuixTheme.textStyles.body1,
                                                            fontWeight = FontWeight.Bold,
                                                        )
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Spacer(Modifier.height(12.dp))
                    TextButton(
                        text = if (refreshing) "刷新中…" else "手动刷新积分",
                        enabled = !refreshing,
                        onClick = { refreshCredits() },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        }
    }
}

// ── 账号 ─────────────────────────────────────────────────────────────────────

@Composable
fun AccountsPage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var accounts by remember { mutableStateOf(DataCache.accounts) }
        var filter by remember { mutableStateOf<String?>(null) }
        // login flow state (moved from the removed 登录 tab)
        var activeFlow by remember { mutableStateOf<String?>(null) }
        var result by remember { mutableStateOf("") }
        var clearCookies by remember { mutableStateOf(false) }
        var traeUrl by remember { mutableStateOf<String?>(null) }
        var wbState by remember { mutableStateOf<String?>(null) }
        var wbUrl by remember { mutableStateOf<String?>(null) }
        var qoderUrl by remember { mutableStateOf<String?>(null) }
        var showAddSheet by remember { mutableStateOf(false) }

        fun refresh() = scope.launch {
            withContext(Dispatchers.IO) {
                accounts = RootCtl.accounts(filter).map { parseAccount(it) }.also { DataCache.accounts = it }
            }
        }
        LaunchedEffect(filter) { refresh() }

        fun closeFlow() {
            activeFlow = null; traeUrl = null; wbState = null; wbUrl = null; qoderUrl = null
        }

        // start the chosen flow after the account-type picker
        fun startFlow(app: String) {
            scope.launch {
                if (clearCookies) {
                    // CookieManager must be touched on the main thread
                    withContext(Dispatchers.Main) {
                        runCatching {
                            android.webkit.CookieManager.getInstance().apply {
                                removeAllCookies(null); removeSessionCookies(null); flush()
                            }
                        }
                    }
                }
                when (app) {
                    Apps.TRAE -> {
                        val j = withContext(Dispatchers.IO) { RootCtl.traeLoginUrl() }
                        traeUrl = j?.get("url")?.asString
                        if (traeUrl == null) result = "❌ 生成授权链接失败" else activeFlow = "trae"
                    }
                    Apps.WB -> {
                        val j = withContext(Dispatchers.IO) { RootCtl.wbStart() }
                        wbState = j?.get("state")?.asString
                        wbUrl = j?.get("auth_url")?.asString
                        if (wbUrl == null) result = "❌ 获取登录链接失败" else activeFlow = "wb"
                    }
                    Apps.QODER -> {
                        val j = withContext(Dispatchers.IO) { RootCtl.call("qoder-start").json() }
                        qoderUrl = j?.get("auth_url")?.asString
                        if (qoderUrl == null) result = "❌ 发起设备流失败" else activeFlow = "qoder"
                    }
                }
            }
        }

        // Full-screen login webview (same behavior as the removed tab)
        if (activeFlow != null) {
            EmbeddedLoginWebview(
                url = traeUrl ?: wbUrl ?: qoderUrl.orEmpty(),
                title = when (activeFlow) {
                    "trae" -> "Trae 登录"
                    "wb" -> "WorkBuddy 登录"
                    else -> "Qoder 授权"
                },
                onClose = { closeFlow() },
                onTraeCallback = { cbUrl ->
                    scope.launch {
                        val r = withContext(Dispatchers.IO) { RootCtl.traeCallback(cbUrl) }
                        val j = r.json()
                        result = when {
                            j?.get("ok")?.asBoolean == true ->
                                "✅ Trae 登录成功：${j?.get("name")?.asString ?: j?.get("user_id")?.asString}"
                            else -> "❌ ${j?.get("error")?.asString ?: r.stdout.take(160)}"
                        }
                        closeFlow()
                        refresh()
                    }
                },
                onWbFinish = {
                    scope.launch {
                        val st = wbState
                        if (st == null) { result = "❌ 会话丢失，请重试"; closeFlow(); return@launch }
                        var last = ""
                        repeat(12) {
                            val r = withContext(Dispatchers.IO) { RootCtl.wbPoll(st) }
                            val j = r.json()
                            if (j?.get("pending")?.asBoolean == true) {
                                last = "等待登录完成…"
                                kotlinx.coroutines.delay(3000)
                            } else {
                                result = when {
                                    j?.get("ok")?.asBoolean == true -> "✅ WorkBuddy 登录成功：${j?.get("user_id")?.asString}"
                                    else -> "❌ ${r.stdout.take(160)}"
                                }
                                closeFlow()
                                refresh()
                                return@launch
                            }
                        }
                        result = "❌ 轮询超时（$last）"
                        closeFlow()
                    }
                },
                onQoderFinish = {
                    scope.launch {
                        val r = withContext(Dispatchers.IO) { RootCtl.qoderWait(240) }
                        val lines = r.stdout.lines().filter { it.trim().startsWith("{") }
                        val j = lines.lastOrNull()?.let {
                            runCatching { com.google.gson.JsonParser.parseString(it).asJsonObject }.getOrNull()
                        }
                        result = when {
                            j?.has("ok") == true && j.get("ok").asBoolean -> "✅ Qoder 授权成功：${j.get("user_id")?.asString ?: ""}"
                            j?.has("error") == true -> "❌ ${j.get("error").asString}"
                            else -> "❌ ${r.stdout.take(160)}"
                        }
                        closeFlow()
                        refresh()
                    }
                },
            )
            return@StatusGate
        }

        Box(Modifier.fillMaxSize()) {
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 12.dp, bottom = 160.dp)) {
                item {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(bottom = 4.dp)) {
                        CharChip("全部", filter == null) { filter = null }
                        Apps.ALL.forEach { app -> CharChip(Apps.displayName(app), filter == app) { filter = app } }
                    }
                    if (result.isNotEmpty()) {
                        Card(modifier = Modifier.fillMaxWidth().padding(bottom = 10.dp)) {
                            Column(Modifier.padding(16.dp)) {
                                Text(result, style = MiuixTheme.textStyles.body2)
                                Spacer(Modifier.height(6.dp))
                                TextButton(text = "清除提示", onClick = { result = "" })
                            }
                        }
                    }
                }
                items(accounts, key = { it.app + it.userId }) { a ->
                    Card(modifier = Modifier.fillMaxWidth().padding(bottom = 10.dp)) {
                        Column(Modifier.padding(start = 16.dp, bottom = 12.dp, top = 16.dp, end = 16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                                Text(a.name.ifEmpty { a.userId }, style = MiuixTheme.textStyles.body1, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
                                // enable/disable switch at the card's top-right corner
                                Switch(
                                    checked = a.enabled,
                                    onCheckedChange = { checked ->
                                        scope.launch {
                                            withContext(Dispatchers.IO) { RootCtl.setEnabled(a.app, a.userId, checked) }
                                            refresh()
                                        }
                                    },
                                )
                            }
                            Text(
                                Apps.displayName(a.app) + " · " + listOfNotNull(
                                    if (a.enabled) "已启用" else "已停用",
                                    when (a.jwtStatus) {
                                        "ok" -> "JWT 正常"
                                        "warn" -> "JWT ${"%.0f".format(a.jwtExpHours ?: 0.0)}h"
                                        "expired" -> "JWT 已过期"
                                        else -> null
                                    },
                                    if (a.needsRelogin) "需重登" else null,
                                ).joinToString(" · "),
                                style = MiuixTheme.textStyles.footnote1,
                                color = when {
                                    a.needsRelogin || a.jwtStatus == "expired" -> MiuixTheme.colorScheme.error
                                    else -> MiuixTheme.colorScheme.onSurfaceVariantSummary
                                },
                            )
                            a.deviceId?.let {
                                Text("指纹 $it", style = MiuixTheme.textStyles.footnote2, fontFamily = FontFamily.Monospace, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                            }
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                if (a.app == Apps.TRAE) {
                                    TextButton(
                                        text = "刷新 JWT",
                                        onClick = {
                                            scope.launch {
                                                withContext(Dispatchers.IO) { RootCtl.traeRefresh(a.userId) }
                                                refresh()
                                            }
                                        },
                                    )
                                }
                                TextButton(
                                    text = "删除",
                                    onClick = {
                                        scope.launch {
                                            withContext(Dispatchers.IO) { RootCtl.remove(a.app, a.userId) }
                                            refresh()
                                        }
                                    },
                                )
                            }
                        }
                    }
                }
                if (accounts.isEmpty()) item {
                    Text("暂无账号，点右下角 + 添加", style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                }
            }

            // add-account FAB, bottom-right above the glass bar (left corner
            // overlapped the bar — user report)
            FloatingActionButton(
                onClick = { showAddSheet = true },
                modifier = Modifier
                    .align(Alignment.BottomEnd)
                    .padding(end = 20.dp, bottom = 24.dp),
            ) {
                Icon(
                    imageVector = MiuixTabIcons.add,
                    contentDescription = "添加账号",
                    tint = MiuixTheme.colorScheme.onPrimary,
                )
            }

            // account-type picker sheet (window-level: OverlayBottomSheet crashed
            // with "No NavigationEventDispatcher" outside a miuix Scaffold host)
            WindowBottomSheet(
                show = showAddSheet,
                title = "添加账号",
                onDismissRequest = { showAddSheet = false },
            ) {
                Column(Modifier.padding(bottom = 16.dp)) {
                    SwitchPreference(
                        title = "清除原有 Cookie",
                        summary = "登录前清空 WebView 已保存的 Cookie（避免旧登录态串号，推荐多账号时开启）",
                        checked = clearCookies,
                        onCheckedChange = { clearCookies = it },
                    )
                    Spacer(Modifier.height(8.dp))
                    listOf(
                        Triple(Apps.TRAE, "Trae · OAuth 授权", "在 App 内完成登录，自动接收凭证"),
                        Triple(Apps.WB, "WorkBuddy · 扫码登录", "内嵌打开登录页，完成后点「我已登录」"),
                        Triple(Apps.QODER, "Qoder · 设备流授权", "内嵌打开授权页，后台自动轮询（最长 240 秒）"),
                    ).forEach { (app, title, summary) ->
                        Card(modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
                            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                Text(title, style = MiuixTheme.textStyles.body1, fontWeight = FontWeight.Bold)
                                Text(summary, style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                                TextButton(
                                    text = "开始登录",
                                    onClick = {
                                        showAddSheet = false
                                        startFlow(app)
                                    },
                                    modifier = Modifier.fillMaxWidth(),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun CharChip(label: String, selected: Boolean, onClick: () -> Unit) {
    TextButton(
        text = label,
        onClick = onClick,
        colors = if (selected) ButtonDefaults.textButtonColorsPrimary() else ButtonDefaults.textButtonColors(),
    )
}

/** Full-screen embedded webview with a Miuix-style top bar and a finish button
 *  for poll-based flows. Trae needs no button — the redirect intercept finishes it. */
@Composable
private fun EmbeddedLoginWebview(
    url: String,
    title: String,
    onClose: () -> Unit,
    onTraeCallback: (String) -> Unit,
    onWbFinish: () -> Unit,
    onQoderFinish: () -> Unit,
) {
    val isTrae = title.startsWith("Trae")
    val isWb = title.startsWith("WorkBuddy")
    val context = RootCtlAppContextHolder.context
    var progress by remember(url) { mutableStateOf(0) }
    var doneLoading by remember(url) { mutableStateOf(false) }
    // diagnostics for blank pages (Qoder): surface the last failed URL + error
    var lastError by remember(url) { mutableStateOf("") }
    var currentUrl by remember(url) { mutableStateOf(url) }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TextButton(text = "关闭", onClick = onClose)
            Text(title, style = MiuixTheme.textStyles.body1, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            // escape hatch: some auth pages (Qoder 白屏) render fine in a real browser
            TextButton(
                text = "浏览器打开",
                onClick = {
                    runCatching {
                        val i = android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(currentUrl))
                        i.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                        androidx.core.content.ContextCompat.startActivity(context, i, null)
                    }
                },
            )
            if (!isTrae) {
                TextButton(
                    text = "我已登录",
                    onClick = { if (isWb) onWbFinish() else onQoderFinish() },
                )
            }
        }
        if (!doneLoading) {
            LinearProgressIndicator(
                progress = if (progress in 1..99) progress / 100f else null,
                modifier = Modifier.fillMaxWidth(),
            )
        }
        if (lastError.isNotEmpty()) {
            Text(
                "页面加载失败：$lastError\n可点「浏览器打开」在外部浏览器完成授权后回来点「我已登录」。",
                style = MiuixTheme.textStyles.footnote2,
                color = MiuixTheme.colorScheme.error,
                modifier = Modifier.padding(horizontal = 12.dp),
            )
        }
        val webView = remember(url) {
            android.webkit.WebView(context).apply {
                settings.javaScriptEnabled = true
                settings.domStorageEnabled = true
                settings.databaseEnabled = true
                // Qoder 白屏对策：授权页前端对 UA 做环境检测，mobile UA 会被判
                // 「不支持的环境」渲染空白 —— 报桌面 Chrome UA（与桌面版同源）
                settings.userAgentString = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
                settings.mixedContentMode = android.webkit.WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
                webViewClient = object : android.webkit.WebViewClient() {
                    override fun shouldOverrideUrlLoading(
                        view: android.webkit.WebView,
                        request: android.webkit.WebResourceRequest,
                    ): Boolean {
                        val u = request.url
                        // Trae: intercept the loopback redirect — trae-callback
                        // accepts the full URL verbatim (same finish path as trae-wait).
                        if (u.host == "127.0.0.1" || u.host == "localhost") {
                            onTraeCallback(u.toString())
                            view.stopLoading()
                            return true
                        }
                        // non-http(s) schemes (weixin:// etc.) cannot render in a
                        // webview — hand them to the system
                        if (u.scheme != "https" && u.scheme != "http") {
                            runCatching {
                                context.startActivity(
                                    android.content.Intent(android.content.Intent.ACTION_VIEW, u)
                                        .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                                )
                            }
                            return true
                        }
                        currentUrl = u.toString()
                        return false
                    }
                    override fun onReceivedError(
                        view: android.webkit.WebView,
                        request: android.webkit.WebResourceRequest,
                        error: android.webkit.WebResourceError,
                    ) {
                        // only surface main-frame failures (favicon 404s are noise)
                        if (request.isForMainFrame) {
                            lastError = "${error.description} (code=${error.errorCode}) — $currentUrl"
                        }
                    }
                    override fun onPageFinished(view: android.webkit.WebView, u: String?) {
                        doneLoading = true
                        // blank-page detector: main frame finished but rendered
                        // nothing → show a hint instead of a silent white screen
                        if (u != null && view.title.isNullOrBlank() && view.contentHeight == 0) {
                            lastError = "页面渲染为空（可能需要外部浏览器）— $u"
                        }
                    }
                }
                webChromeClient = object : android.webkit.WebChromeClient() {
                    override fun onProgressChanged(view: android.webkit.WebView?, newProgress: Int) {
                        progress = newProgress
                    }
                }
                loadUrl(url)
            }
        }
        AndroidView(
            factory = { webView },
            modifier = Modifier.fillMaxSize(),
        )
        DisposableEffect(url) {
            onDispose { webView.destroy() }
        }
    }
}

object RootCtlAppContextHolder {
    lateinit var context: android.content.Context
}

/** Process-level data cache — tab switches render from the last fetch instead
 *  of re-running the (slow, network-bound) module CLI each time. */
object DataCache {
    @Volatile var credits: CreditsSnapshot? = null
    @Volatile var status: StatusView? = null
    @Volatile var accounts: List<AccountRow> = emptyList()
}

// ── 日志 ─────────────────────────────────────────────────────────────────────

/** NDJSON event → "[2026-10-17 12:34:56] [done] ok=1 already=0 failed=0" style.
 *  Falls back to the raw line for non-JSON payloads. */
private fun formatLogLine(raw: String): String {
    val o = runCatching { com.google.gson.JsonParser.parseString(raw).asJsonObject }.getOrNull() ?: return raw
    // legacy lines (pre-0.2.3 daemon) carry no ts — show them undated instead
    // of a "????-??-??" placeholder (user complaint: 问号)
    val ts = o.get("ts")?.takeIf { it.isJsonPrimitive }?.asString?.take(19)?.replace('T', ' ')
    val type = o.get("type")?.takeIf { it.isJsonPrimitive }?.asString ?: "event"
    val details = o.entrySet()
        .filter { it.key !in setOf("type", "ts") }
        .joinToString(" ") { (k, v) ->
            val value = when {
                v.isJsonNull -> "-"
                v.isJsonObject || v.isJsonArray -> "…"
                !v.isJsonPrimitive -> "…"
                else -> v.asString.ifEmpty { "-" }
            }
            "$k=$value"
        }
    return (if (ts != null) "[$ts] " else "") + "[$type] $details"
}

@Composable
fun LogPage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var lines by remember { mutableStateOf<List<String>>(emptyList()) }
        LaunchedEffect(Unit) {
            withContext(Dispatchers.IO) { lines = RootCtl.log(300) }
        }
        Column(Modifier.fillMaxSize().padding(start = 12.dp, end = 12.dp, top = 12.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                Text("事件日志", style = MiuixTheme.textStyles.title3)
                TextButton(
                    text = "刷新",
                    onClick = { scope.launch { withContext(Dispatchers.IO) { lines = RootCtl.log(300) } } },
                )
            }
            LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 96.dp)) {
                items(lines.reversed()) { raw ->
                    // render "[ts] line": extract the ts field from the NDJSON event
                    // color pinned to theme onSurface (default Text uses onPrimary,
                    // invisible on the theme background in dark mode — user complaint)
                    val line = formatLogLine(raw)
                    Text(
                        line,
                        style = MiuixTheme.textStyles.footnote2,
                        fontFamily = FontFamily.Monospace,
                        color = MiuixTheme.colorScheme.onSurface,
                        modifier = Modifier.padding(vertical = 1.dp),
                    )
                }
                if (lines.isEmpty()) item { Text("暂无日志（等待守护进程执行签到）", style = MiuixTheme.textStyles.body2) }
            }
        }
    }
}

// ── 设置 ─────────────────────────────────────────────────────────────────────

@Composable
fun SettingsPage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var status by remember { mutableStateOf<StatusView?>(null) }
        var saved by remember { mutableStateOf("") }
        var jitter by remember(status) { mutableStateOf(status?.jitter ?: true) }
        var flavor by remember(status) { mutableStateOf(status?.platformFlavor ?: "windows") }
        var windowStart by remember(status) { mutableStateOf(((status?.windowStart ?: 480) / 60).toString() + ":" + "%02d".format((status?.windowStart ?: 480) % 60)) }
        var windowEnd by remember(status) { mutableStateOf(((status?.windowEnd ?: 600) / 60).toString() + ":" + "%02d".format((status?.windowEnd ?: 600) % 60)) }
        var gap by remember(status) { mutableStateOf((status?.accountGap ?: 3).toString()) }

        LaunchedEffect(Unit) {
            withContext(Dispatchers.IO) { status = RootCtl.status()?.let { parseStatus(it) } }
        }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 12.dp, bottom = 96.dp)) {
            item {
                SmallTitle(text = "签到调度")
                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column {
                        BasicRow("签到窗口", "格式 HH:mm，例如 8:00 与 10:00")
                        TextField(value = windowStart, onValueChange = { windowStart = it }, label = "开始 (HH:mm)", modifier = Modifier.padding(horizontal = 16.dp))
                        TextField(value = windowEnd, onValueChange = { windowEnd = it }, label = "结束 (HH:mm)", modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = 8.dp))
                        SwitchPreference(
                            title = "随机抖动",
                            summary = "在窗口内按天确定性选点，三应用选点互不相同（防风控）",
                            checked = jitter,
                            onCheckedChange = { jitter = it },
                        )
                        BasicRow("账号间隔（秒）", "同应用多账号串行等待")
                        TextField(value = gap, onValueChange = { gap = it.filter { c -> c.isDigit() } }, label = "秒", modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = 8.dp))
                    }
                }
                SmallTitle(text = "平台指纹")
                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column {
                        val flavorItems = listOf("windows", "android")
                        val flavorIdx = flavorItems.indexOf(flavor).coerceAtLeast(0)
                        WindowDropdownPreference(
                            items = flavorItems,
                            selectedIndex = flavorIdx,
                            title = "指纹模式",
                            summary = "windows = 与桌面版请求头完全一致（默认，新 JWT 校验设备指纹时最稳）；android = 实验逃生通道",
                            onSelectedIndexChange = { flavor = flavorItems[it] },
                        )
                    }
                }
                TextButton(
                    text = "保存设置",
                    onClick = {
                        fun parseHM(s: String, fallback: Int): Int {
                            val parts = s.split(":")
                            val h = parts.getOrNull(0)?.trim()?.toIntOrNull() ?: return fallback
                            val m = parts.getOrNull(1)?.trim()?.toIntOrNull() ?: 0
                            return (h * 60 + m).coerceIn(0, 24 * 60)
                        }
                        val startMin = parseHM(windowStart, 480)
                        val endMin = parseHM(windowEnd, 600)
                        val patch = """{"window_start_min":$startMin,"window_end_min":$endMin,""" +
                            """"account_gap_secs":${gap.toIntOrNull() ?: 3},"jitter":$jitter,"platform_flavor":"$flavor"}"""
                        scope.launch {
                            val r = withContext(Dispatchers.IO) { RootCtl.setConfig(patch) }
                            saved = if (r.code == 0) "已保存" else "保存失败: ${r.stdout.take(120)}"
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                )
                if (saved.isNotEmpty()) {
                    Spacer(Modifier.height(6.dp))
                    Text(saved, style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.primary)
                }
                Spacer(Modifier.height(8.dp))
                Text(
                    "说明：windows 指纹 = 默认（与桌面版请求头完全一致；新登录 JWT 校验设备指纹时最稳）。" +
                        "android 指纹仅作为实验逃生通道，若服务端拒绝请切回 windows。",
                    style = MiuixTheme.textStyles.footnote1,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                )
            }
        }
    }
}

@Composable
private fun BasicRow(title: String, summary: String) {
    Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
        Text(title, style = MiuixTheme.textStyles.body1)
        Text(summary, style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
    }
}
