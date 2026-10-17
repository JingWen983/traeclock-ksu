package cn.traeclock.manager

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
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
import top.yukonga.miuix.kmp.basic.LinearProgressIndicator
import top.yukonga.miuix.kmp.basic.NavigationBar
import top.yukonga.miuix.kmp.basic.NavigationBarItem
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.SmallTitle
import top.yukonga.miuix.kmp.basic.SmallTopAppBar
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.preference.SwitchPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme

class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        RootCtlAppContextHolder.context = applicationContext
        setContent { TraeClockApp() }
    }
}

@Composable
fun TraeClockApp() {
    val controller = remember { top.yukonga.miuix.kmp.theme.ThemeController(top.yukonga.miuix.kmp.theme.ColorSchemeMode.System) }
    top.yukonga.miuix.kmp.theme.MiuixTheme(controller = controller) {
        var tab by remember { mutableIntStateOf(0) }
        val tabs = listOf("概览", "看板", "账号", "登录", "日志", "设置")
        Scaffold(
            topBar = { SmallTopAppBar(title = "TraeClock", color = MiuixTheme.colorScheme.background) },
            bottomBar = {
                NavigationBar {
                    tabs.forEachIndexed { index, label ->
                        NavigationBarItem(
                            selected = tab == index,
                            onClick = { tab = index },
                            icon = MiuixTabIcons.forIndex(index),
                            label = label,
                        )
                    }
                }
            },
        ) { innerPadding ->
            Box(Modifier.padding(innerPadding)) {
                when (tab) {
                    0 -> OverviewPage()
                    1 -> CreditsPage()
                    2 -> AccountsPage()
                    3 -> LoginPage()
                    4 -> LogPage()
                    5 -> SettingsPage()
                }
            }
        }
    }
}

/** Tab icons: Compose material core icons (small dependency, sufficient here). */
private object MiuixTabIcons {
    val forIndex: (Int) -> androidx.compose.ui.graphics.vector.ImageVector = { index ->
        when (index) {
            0 -> androidx.compose.material.icons.Icons.Filled.Home
            1 -> androidx.compose.material.icons.Icons.Filled.Info
            2 -> androidx.compose.material.icons.Icons.Filled.List
            3 -> androidx.compose.material.icons.Icons.Filled.Add
            4 -> androidx.compose.material.icons.Icons.Filled.DateRange
            else -> androidx.compose.material.icons.Icons.Filled.Check
        }
    }
}

// ── shared page scaffold (status gate) ───────────────────────────────────────

@Composable
fun StatusGate(content: @Composable (refresh: () -> Unit) -> Unit) {
    val scope = rememberCoroutineScope()
    var rootOk by remember { mutableStateOf<Boolean?>(null) }
    var moduleOk by remember { mutableStateOf<Boolean?>(null) }

    fun check() = scope.launch {
        withContext(Dispatchers.IO) {
            rootOk = RootCtl.isRootAvailable()
            moduleOk = rootOk == true && RootCtl.isModuleInstalled()
        }
    }
    LaunchedEffect(Unit) { check() }

    when {
        rootOk == false -> GateCard("未获得 Root", "本应用需要 KernelSU/Magisk root 才能与模块通信。")
        moduleOk == false -> GateCard("未检测到 TraeClock 模块", "请先在 KernelSU 管理器中刷入 TraeClock 模块 zip 并重启。")
        else -> content { check() }
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

// ── 概览 ─────────────────────────────────────────────────────────────────────

@Composable
fun OverviewPage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var status by remember { mutableStateOf<StatusView?>(null) }
        var busy by remember { mutableStateOf(false) }
        var msg by remember { mutableStateOf("") }

        fun refresh() = scope.launch {
            withContext(Dispatchers.IO) { status = RootCtl.status()?.let { parseStatus(it) } }
        }
        LaunchedEffect(Unit) { refresh() }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(12.dp)) {
            item {
                val s = status
                if (s == null) {
                    Text("加载中…", style = MiuixTheme.textStyles.body2)
                } else {
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
                                refresh()
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        }
    }
}

// ── 实时积分看板 ──────────────────────────────────────────────────────────────

@Composable
fun CreditsPage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var snapshot by remember { mutableStateOf<CreditsSnapshot?>(null) }
        var refreshing by remember { mutableStateOf(false) }
        var expanded by remember { mutableStateOf(setOf<String>()) }

        fun refresh() = scope.launch {
            refreshing = true
            withContext(Dispatchers.IO) { snapshot = RootCtl.credits()?.let { parseCredits(it) } }
            refreshing = false
        }
        LaunchedEffect(Unit) { refresh() }
        // auto refresh while the page stays open (real-time dashboard)
        LaunchedEffect(Unit) {
            while (true) {
                kotlinx.coroutines.delay(60_000)
                withContext(Dispatchers.IO) { snapshot = RootCtl.credits()?.let { parseCredits(it) } }
            }
        }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(12.dp)) {
            item {
                val snap = snapshot
                if (snap == null) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        LinearProgressIndicator(modifier = Modifier.weight(1f))
                        Text("查询中…", style = MiuixTheme.textStyles.body2)
                    }
                } else {
                    SmallTitle(text = "总积分")
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
                        text = if (refreshing) "刷新中…" else "手动刷新",
                        enabled = !refreshing,
                        onClick = { refresh() },
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
        var accounts by remember { mutableStateOf<List<AccountRow>>(emptyList()) }
        var filter by remember { mutableStateOf<String?>(null) }

        fun refresh() = scope.launch {
            withContext(Dispatchers.IO) {
                accounts = RootCtl.accounts(filter).map { parseAccount(it) }
            }
        }
        LaunchedEffect(filter) { refresh() }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(12.dp)) {
            item {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(bottom = 4.dp)) {
                    CharChip("全部", filter == null) { filter = null }
                    Apps.ALL.forEach { app -> CharChip(Apps.displayName(app), filter == app) { filter = app } }
                }
            }
            items(accounts, key = { it.app + it.userId }) { a ->
                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 10.dp)) {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                            Text(a.name.ifEmpty { a.userId }, style = MiuixTheme.textStyles.body1, fontWeight = FontWeight.Bold)
                            Text(Apps.displayName(a.app), style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                        }
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                            Text(
                                listOfNotNull(
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
                        }
                        a.deviceId?.let {
                            Text("指纹 $it", style = MiuixTheme.textStyles.footnote2, fontFamily = FontFamily.Monospace, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton(
                                text = if (a.enabled) "停用" else "启用",
                                onClick = {
                                    scope.launch {
                                        withContext(Dispatchers.IO) { RootCtl.setEnabled(a.app, a.userId, !a.enabled) }
                                        refresh()
                                    }
                                },
                            )
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
                Text("暂无账号，请到「登录」页添加", style = MiuixTheme.textStyles.body2, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
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

// ── 登录（三应用向导）────────────────────────────────────────────────────────

@Composable
fun LoginPage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var traeResult by remember { mutableStateOf("") }
        var callbackInput by remember { mutableStateOf("") }
        var wbStateId by remember { mutableStateOf<String?>(null) }
        var wbResult by remember { mutableStateOf("") }
        var qoderBusy by remember { mutableStateOf(false) }
        var qoderResult by remember { mutableStateOf("") }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(12.dp)) {
            item {
                SmallTitle(text = "添加账号")
                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Trae · OAuth 授权登录", style = MiuixTheme.textStyles.body1, fontWeight = FontWeight.Bold)
                        Text(
                            "① 点开始 → 自动打开浏览器并监听回调；② 在浏览器完成登录；③ 回到 App 看结果。",
                            style = MiuixTheme.textStyles.footnote1,
                            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                        )
                        var traeBusy by remember { mutableStateOf(false) }
                        TextButton(
                            text = if (traeBusy) "等待回调中…" else "开始登录（自动接收回调）",
                            enabled = !traeBusy,
                            onClick = {
                                traeBusy = true; traeResult = ""
                                scope.launch {
                                    val waitJob = async(Dispatchers.IO) { RootCtl.traeWait(300) }
                                    withContext(Dispatchers.IO) {
                                        RootCtl.traeLoginUrl()?.get("url")?.asString?.let { openBrowser(it) }
                                    }
                                    val r = waitJob.await(); traeBusy = false
                                    val j = runCatching { com.google.gson.JsonParser.parseString(r.stdout).asJsonObject }.getOrNull()
                                    traeResult = when {
                                        j?.has("ok") == true && j.get("ok").asBoolean -> "成功：${j.get("user_id")?.asString ?: ""}"
                                        j?.has("error") == true -> j.get("error").asString
                                        else -> r.stdout.take(200)
                                    }
                                }
                            },
                            modifier = Modifier.fillMaxWidth(),
                        )
                        if (traeResult.isNotEmpty()) Text("结果: $traeResult", style = MiuixTheme.textStyles.footnote1)
                        Text("备用：复制浏览器最终 127.0.0.1 页面地址手动完成。", style = MiuixTheme.textStyles.footnote2, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                        TextField(
                            value = callbackInput,
                            onValueChange = { callbackInput = it },
                            label = "手动粘贴回调 URL（备用）",
                        )
                        TextButton(
                            text = "手动完成登录",
                            enabled = callbackInput.contains("127.0.0.1"),
                            onClick = {
                                scope.launch {
                                    val r = withContext(Dispatchers.IO) { RootCtl.traeCallback(callbackInput.trim()) }
                                    traeResult = r.json()?.get("ok")?.asBoolean?.let { "成功" } ?: r.stdout.take(200)
                                }
                            },
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }

                Card(modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("WorkBuddy · 扫码登录", style = MiuixTheme.textStyles.body1, fontWeight = FontWeight.Bold)
                        Text("① 获取登录链接并打开；② 完成扫码/登录后回来点轮询。", style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                        TextButton(
                            text = "① 获取登录链接并打开",
                            onClick = {
                                scope.launch {
                                    val j = withContext(Dispatchers.IO) { RootCtl.wbStart() }
                                    wbStateId = j?.get("state")?.asString
                                    j?.get("auth_url")?.asString?.let { openBrowser(it) }
                                }
                            },
                            modifier = Modifier.fillMaxWidth(),
                        )
                        if (wbStateId != null) {
                            TextButton(
                                text = "② 我已登录，轮询结果",
                                onClick = {
                                    scope.launch {
                                        val r = withContext(Dispatchers.IO) { RootCtl.wbPoll(wbStateId!!) }
                                        val j = r.json()
                                        wbResult = when {
                                            j?.get("pending")?.asBoolean == true -> "等待登录完成…"
                                            j?.get("ok")?.asBoolean == true -> "成功：${j?.get("user_id")?.asString}"
                                            else -> r.stdout.take(200)
                                        }
                                    }
                                },
                                modifier = Modifier.fillMaxWidth(),
                            )
                        }
                        if (wbResult.isNotEmpty()) Text(wbResult, style = MiuixTheme.textStyles.footnote1)
                    }
                }

                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("Qoder · 设备流授权", style = MiuixTheme.textStyles.body1, fontWeight = FontWeight.Bold)
                        Text("① 点开始自动打开授权页（后台轮询 180 秒，可锁屏）；若提示「参数无效」再点一次切兼容模式。", style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                        TextButton(
                            text = if (qoderBusy) "等待授权…" else "① 开始设备流授权",
                            enabled = !qoderBusy,
                            onClick = {
                                qoderBusy = true; qoderResult = "等待授权…"
                                scope.launch {
                                    val r = withContext(Dispatchers.IO) { RootCtl.qoderWait(180) }
                                    qoderBusy = false
                                    val lines = r.stdout.lines().filter { it.trim().startsWith("{") }
                                    val started = lines.firstOrNull()?.let {
                                        runCatching { com.google.gson.JsonParser.parseString(it).asJsonObject }.getOrNull()
                                    }
                                    started?.get("auth_url")?.asString?.let { openBrowser(it) }
                                    val j = lines.lastOrNull()?.let {
                                        runCatching { com.google.gson.JsonParser.parseString(it).asJsonObject }.getOrNull()
                                    }
                                    qoderResult = when {
                                        j?.has("ok") == true && j.get("ok").asBoolean -> "成功：${j.get("user_id")?.asString ?: ""}"
                                        j?.has("error") == true -> j.get("error").asString
                                        else -> r.stdout.take(200)
                                    }
                                }
                            },
                            modifier = Modifier.fillMaxWidth(),
                        )
                        if (qoderResult.isNotEmpty()) Text(qoderResult, style = MiuixTheme.textStyles.footnote1)
                    }
                }
            }
        }
    }
}

fun openBrowser(url: String) {
    runCatching {
        val intent = android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url))
        intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
        androidx.core.content.ContextCompat.startActivity(RootCtlAppContextHolder.context, intent, null)
    }
}

object RootCtlAppContextHolder {
    lateinit var context: android.content.Context
}

// ── 日志 ─────────────────────────────────────────────────────────────────────

@Composable
fun LogPage() {
    StatusGate { _ ->
        val scope = rememberCoroutineScope()
        var lines by remember { mutableStateOf<List<String>>(emptyList()) }
        LaunchedEffect(Unit) {
            withContext(Dispatchers.IO) { lines = RootCtl.log(300) }
        }
        Column(Modifier.fillMaxSize().padding(12.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                Text("事件日志", style = MiuixTheme.textStyles.title3)
                TextButton(
                    text = "刷新",
                    onClick = { scope.launch { withContext(Dispatchers.IO) { lines = RootCtl.log(300) } } },
                )
            }
            LazyColumn {
                items(lines.reversed()) { l ->
                    Text(l, style = MiuixTheme.textStyles.footnote2, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(vertical = 1.dp))
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
        var flavor by remember(status) { mutableStateOf(status?.platformFlavor ?: "android") }
        var windowStart by remember(status) { mutableStateOf(((status?.windowStart ?: 480) / 60).toString() + ":" + "%02d".format((status?.windowStart ?: 480) % 60)) }
        var windowEnd by remember(status) { mutableStateOf(((status?.windowEnd ?: 600) / 60).toString() + ":" + "%02d".format((status?.windowEnd ?: 600) % 60)) }
        var gap by remember(status) { mutableStateOf((status?.accountGap ?: 3).toString()) }

        LaunchedEffect(Unit) {
            withContext(Dispatchers.IO) { status = RootCtl.status()?.let { parseStatus(it) } }
        }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(12.dp)) {
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
                        BasicRow("指纹模式", if (flavor == "android") "android（手机默认）" else "windows（与桌面一致，逃生通道）")
                        Row(Modifier.padding(horizontal = 16.dp).padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CharChip("android", flavor == "android") { flavor = "android" }
                            CharChip("windows", flavor == "windows") { flavor = "windows" }
                        }
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
                    "说明：android 指纹 = 手机端默认（x-lscbd-platform / cosy 系列换 android 值）；" +
                        "如遇服务端拒绝可切回 windows 指纹（与桌面版完全一致）。",
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
