package cn.traeclock.manager

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.*
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

class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        RootCtlAppContextHolder.context = applicationContext
        setContent { TraeClockApp() }
    }
}

@Composable
fun TraeClockApp() {
    MaterialTheme(colorScheme = lightColorScheme()) {
        var tab by remember { mutableIntStateOf(0) }
        val tabs = listOf("概览", "账号", "登录", "日志", "设置")
        Scaffold(
            bottomBar = {
                NavigationBar {
                    tabs.forEachIndexed { i, t ->
                        NavigationBarItem(
                            selected = tab == i,
                            onClick = { tab = i },
                            label = { Text(t) },
                            icon = { Icon(Icons.Default.PlayArrow, contentDescription = t) },
                        )
                    }
                }
            }
        ) { pad ->
            Box(Modifier.padding(pad)) {
                when (tab) {
                    0 -> OverviewPage()
                    1 -> AccountsPage()
                    2 -> LoginPage()
                    3 -> LogPage()
                    4 -> SettingsPage()
                }
            }
        }
    }
}

// ── 概览 ─────────────────────────────────────────────────────────────────────

@Composable
fun OverviewPage() {
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf<StatusView?>(null) }
    var rootOk by remember { mutableStateOf<Boolean?>(null) }
    var moduleOk by remember { mutableStateOf<Boolean?>(null) }
    var busy by remember { mutableStateOf(false) }
    var msg by remember { mutableStateOf("") }

    fun refresh() = scope.launch {
        withContext(Dispatchers.IO) {
            rootOk = RootCtl.isRootAvailable()
            moduleOk = rootOk == true && RootCtl.isModuleInstalled()
            if (moduleOk == true) status = RootCtl.status()?.let { parseStatus(it) }
        }
    }
    LaunchedEffect(Unit) { refresh() }

    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("TraeClock", fontSize = 22.sp, fontWeight = FontWeight.Bold)
            IconButton(onClick = { refresh() }) { Icon(Icons.Default.Refresh, "刷新") }
        }
        when {
            rootOk == false -> Card { Column(Modifier.padding(16.dp)) {
                Text("未获得 Root", fontWeight = FontWeight.Bold)
                Text("本应用需要 KernelSU/Magisk root 才能与模块通信。")
            } }
            moduleOk == false -> Card { Column(Modifier.padding(16.dp)) {
                Text("未检测到 TraeClock 模块", fontWeight = FontWeight.Bold)
                Text("请先在 KernelSU 管理器中刷入 TraeClock 模块 zip 并重启。")
            } }
            else -> {
                status?.let { s ->
                    Card { Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text("守护进程", fontWeight = FontWeight.Bold)
                        Text(if (s.daemonPid != null) "运行中 (pid ${s.daemonPid})" else "未运行（重启设备后自启）")
                        s.nextApp?.let { app ->
                            Text("下次签到：${Apps.displayName(app)} · ${s.nextInSecs?.let { formatDuration(it) } ?: "—"}")
                        }
                        Text("签到窗口：${s.windowStart / 60}:${"%02d".format(s.windowStart % 60)} – ${s.windowEnd / 60}:${"%02d".format(s.windowEnd % 60)}" + if (s.jitter) "（随机抖动）" else "")
                        Text("账号间隔：${s.accountGap}s · 平台指纹：${s.platformFlavor}")
                    } }
                    Apps.ALL.forEach { app ->
                        val c = s.counts[app] ?: CountRow(0, 0, 0)
                        Card(modifier = Modifier.fillMaxWidth()) {
                            Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                                Text(Apps.displayName(app), fontWeight = FontWeight.Bold)
                                Text("${c.enabled}/${c.total} 启用" + if (c.needsRelogin > 0) " · ${c.needsRelogin} 需重登" else "")
                            }
                        }
                    }
                } ?: Text("加载中…")
                if (msg.isNotEmpty()) Text(msg, color = MaterialTheme.colorScheme.primary)
                Button(
                    enabled = !busy,
                    onClick = {
                        busy = true; msg = "签到中…"
                        scope.launch {
                            val r = withContext(Dispatchers.IO) { RootCtl.runNow(null) }
                            busy = false
                            msg = if (r.code == 0) "已触发全部签到，结果见日志页" else "触发失败: ${r.stdout.take(120)}"
                            refresh()
                        }
                    }
                ) { Text(if (busy) "执行中…" else "立即签到（全部）") }
            }
        }
    }
}

// ── 账号 ─────────────────────────────────────────────────────────────────────

@Composable
fun AccountsPage() {
    val scope = rememberCoroutineScope()
    var accounts by remember { mutableStateOf<List<AccountRow>>(emptyList()) }
    var filter by remember { mutableStateOf<String?>(null) }

    fun refresh() = scope.launch {
        withContext(Dispatchers.IO) {
            accounts = RootCtl.accounts(filter).map { parseAccount(it) }
        }
    }
    LaunchedEffect(filter) { refresh() }

    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = filter == null, onClick = { filter = null }, label = { Text("全部") })
            Apps.ALL.forEach { app ->
                FilterChip(selected = filter == app, onClick = { filter = app }, label = { Text(Apps.displayName(app)) })
            }
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(accounts, key = { it.app + it.userId }) { a ->
                Card {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                            Text(a.name.ifEmpty { a.userId }, fontWeight = FontWeight.Bold)
                            Text(Apps.displayName(a.app), fontSize = 12.sp)
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            AssistChipLabel(if (a.enabled) "已启用" else "已停用")
                            AssistChipLabel(when (a.jwtStatus) {
                                "ok" -> "JWT 正常"
                                "warn" -> "JWT ${"%.0f".format(a.jwtExpHours ?: 0.0)}h"
                                "expired" -> "JWT 已过期"
                                else -> "状态未知"
                            })
                            if (a.needsRelogin) AssistChipLabel("需重新登录", warn = true)
                        }
                        a.deviceId?.let { Text("设备指纹: $it", fontSize = 11.sp, fontFamily = FontFamily.Monospace) }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton(onClick = {
                                scope.launch { withContext(Dispatchers.IO) { RootCtl.setEnabled(a.app, a.userId, !a.enabled) }; refresh() }
                            }) { Text(if (a.enabled) "停用" else "启用") }
                            if (a.app == Apps.TRAE) {
                                TextButton(onClick = {
                                    scope.launch { withContext(Dispatchers.IO) { RootCtl.traeRefresh(a.userId) }; refresh() }
                                }) { Text("刷新 JWT") }
                            }
                            TextButton(onClick = {
                                scope.launch { withContext(Dispatchers.IO) { RootCtl.remove(a.app, a.userId) }; refresh() }
                            }) { Icon(Icons.Default.Delete, "删除", modifier = Modifier.size(18.dp)); Text("删除") }
                        }
                    }
                }
            }
            if (accounts.isEmpty()) item { Text("暂无账号，请到「登录」页添加") }
        }
    }
}

@Composable
fun AssistChipLabel(text: String, warn: Boolean = false) {
    Text(
        text,
        fontSize = 11.sp,
        color = if (warn) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
        fontWeight = FontWeight.Medium,
    )
}

// ── 登录（三应用向导）────────────────────────────────────────────────────────

@Composable
fun LoginPage() {
    val scope = rememberCoroutineScope()
    var traeResult by remember { mutableStateOf("") }
    var callbackInput by remember { mutableStateOf("") }

    var wbStateId by remember { mutableStateOf<String?>(null) }
    var wbUrl by remember { mutableStateOf<String?>(null) }
    var wbResult by remember { mutableStateOf("") }

    var qoderBusy by remember { mutableStateOf(false) }
    var qoderResult by remember { mutableStateOf("") }

    Column(Modifier.fillMaxSize().padding(16.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Text("添加账号", fontSize = 20.sp, fontWeight = FontWeight.Bold)

        // Trae
        Card { Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Trae · OAuth 授权登录", fontWeight = FontWeight.Bold)
            Text("① 点开始 → 自动打开浏览器并监听回调；② 在浏览器完成登录，最终页面提示「登录信息已接收」；③ 回到 App 看结果。", fontSize = 12.sp)
            var traeBusy by remember { mutableStateOf(false) }
            Button(enabled = !traeBusy, onClick = {
                traeBusy = true; traeResult = ""
                scope.launch {
                    // start the loopback listener FIRST, then open the browser;
                    // trae-wait blocks until the callback (max 300s)
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
            }) { Text(if (traeBusy) "等待回调中…" else "① 开始登录（自动接收回调）") }
            if (traeResult.isNotEmpty()) Text("结果: $traeResult", fontSize = 12.sp)
            Text("备用：若自动接收失败，复制浏览器最终 127.0.0.1 页面地址到下面手动完成。", fontSize = 11.sp)
            OutlinedTextField(
                value = callbackInput,
                onValueChange = { callbackInput = it },
                label = { Text("手动粘贴回调 URL（备用）") },
                modifier = Modifier.fillMaxWidth(),
            )
            Button(enabled = callbackInput.contains("127.0.0.1"), onClick = {
                scope.launch {
                    val r = withContext(Dispatchers.IO) { RootCtl.traeCallback(callbackInput.trim()) }
                    traeResult = r.json()?.get("ok")?.asBoolean?.let { "成功" } ?: r.stdout.take(200)
                }
            }) { Text("手动完成登录") }
        } }

        // WorkBuddy
        Card { Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("WorkBuddy · 扫码登录", fontWeight = FontWeight.Bold)
            Text("点击获取登录链接 → 在浏览器完成扫码/登录 → 返回 App 点击轮询直到成功。", fontSize = 12.sp)
            Button(onClick = {
                scope.launch {
                    val j = withContext(Dispatchers.IO) { RootCtl.wbStart() }
                    wbStateId = j?.get("state")?.asString
                    wbUrl = j?.get("auth_url")?.asString
                    wbUrl?.let { openBrowser(it) }
                }
            }) { Text("① 获取登录链接并打开") }
            if (wbStateId != null) {
                Button(onClick = {
                    scope.launch {
                        val r = withContext(Dispatchers.IO) { RootCtl.wbPoll(wbStateId!!) }
                        val j = r.json()
                        wbResult = when {
                            j?.get("pending")?.asBoolean == true -> "等待登录完成…"
                            j?.get("ok")?.asBoolean == true -> "成功：${j?.get("user_id")?.asString}"
                            else -> r.stdout.take(200)
                        }
                    }
                }) { Text("② 我已登录，轮询结果") }
            }
            if (wbResult.isNotEmpty()) Text(wbResult, fontSize = 12.sp)
        } }

        // Qoder
        Card { Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Qoder · 设备流授权", fontWeight = FontWeight.Bold)
            Text("① 点开始 → 自动打开 qoder.cn 授权页（App 后台每秒轮询，最长 180 秒）；② 在浏览器完成授权；③ 可锁屏等待，回 App 看结果。若提示「参数无效」请再点一次（自动切换兼容模式）。", fontSize = 12.sp)
            Button(enabled = !qoderBusy, onClick = {
                qoderBusy = true; qoderResult = "等待授权…"
                scope.launch {
                    val r = withContext(Dispatchers.IO) { RootCtl.qoderWait(180) }
                    qoderBusy = false
                    // trae-wait style: first line is {"stage":"started","auth_url":...},
                    // last line is the result object
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
            }) { Text(if (qoderBusy) "等待授权…" else "① 开始设备流授权") }
            if (qoderResult.isNotEmpty()) Text(qoderResult, fontSize = 12.sp)
        } }
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
    var lines by remember { mutableStateOf<List<String>>(emptyList()) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) { lines = RootCtl.log(300) }
    }
    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("事件日志", fontSize = 18.sp, fontWeight = FontWeight.Bold)
            IconButton(onClick = { scope.launch { withContext(Dispatchers.IO) { lines = RootCtl.log(300) } } }) {
                Icon(Icons.Default.Refresh, "刷新")
            }
        }
        LazyColumn {
            items(lines.reversed()) { l ->
                Text(l, fontSize = 10.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(vertical = 1.dp))
            }
            if (lines.isEmpty()) item { Text("暂无日志（等待守护进程执行签到）") }
        }
    }
}

// ── 设置 ─────────────────────────────────────────────────────────────────────

@Composable
fun SettingsPage() {
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf<StatusView?>(null) }
    var saved by remember { mutableStateOf("") }

    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) { status = RootCtl.status()?.let { parseStatus(it) } }
    }
    var startH by remember(status) { mutableStateOf(((status?.windowStart ?: 480) / 60).toString()) }
    var startM by remember(status) { mutableStateOf(((status?.windowStart ?: 480) % 60).toString()) }
    var endH by remember(status) { mutableStateOf(((status?.windowEnd ?: 600) / 60).toString()) }
    var endM by remember(status) { mutableStateOf(((status?.windowEnd ?: 600) % 60).toString()) }
    var gap by remember(status) { mutableStateOf((status?.accountGap ?: 3).toString()) }
    var flavor by remember(status) { mutableStateOf(status?.platformFlavor ?: "android") }

    Column(Modifier.fillMaxSize().padding(16.dp).verticalScroll(androidx.compose.foundation.rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("设置", fontSize = 18.sp, fontWeight = FontWeight.Bold)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("签到窗口")
            SmallNumberField(startH, "时") { startH = it }; Text(":"); SmallNumberField(startM, "分") { startM = it }
            Text("–")
            SmallNumberField(endH, "时") { endH = it }; Text(":"); SmallNumberField(endM, "分") { endM = it }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("账号间隔(s)"); SmallNumberField(gap, "秒") { gap = it }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = flavor == "android", onClick = { flavor = "android" }, label = { Text("android 指纹") })
            FilterChip(selected = flavor == "windows", onClick = { flavor = "windows" }, label = { Text("windows 指纹") })
        }
        Button(onClick = {
            val patch = """{"window_start_min":${startH.toInt() * 60 + startM.toInt()},""" +
                """"window_end_min":${endH.toInt() * 60 + endM.toInt()},""" +
                """"account_gap_secs":${gap.toInt()},"platform_flavor":"$flavor"}"""
            scope.launch {
                val r = withContext(Dispatchers.IO) { RootCtl.setConfig(patch) }
                saved = if (r.code == 0) "已保存" else "保存失败: ${r.stdout.take(120)}"
            }
        }) { Text("保存") }
        if (saved.isNotEmpty()) Text(saved)
        Text(
            "说明：android 指纹 = 手机端默认（x-lscbd-platform/cosy 系列换 android 值）；" +
                "如遇服务端拒绝可切回 windows 指纹（与桌面版完全一致）。",
            fontSize = 12.sp,
        )
    }
}

@Composable
fun SmallNumberField(value: String, label: String, onValueChange: (String) -> Unit = {}) {
    var v by remember(value) { mutableStateOf(value) }
    OutlinedTextField(
        value = v,
        onValueChange = { nv -> v = nv.filter { it.isDigit() }; onValueChange(v) },
        label = { Text(label, fontSize = 10.sp) },
        modifier = Modifier.width(72.dp),
        textStyle = LocalTextStyle.current.copy(fontSize = 13.sp),
        singleLine = true,
    )
}
