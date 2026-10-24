package cn.traeclock.manager

import com.google.gson.Gson
import com.google.gson.JsonObject
import com.google.gson.JsonParser
import com.topjohnwu.superuser.Shell

/** One CLI invocation result. */
data class CtlResult(val code: Int, val stdout: String) {
    fun json(): JsonObject? = runCatching {
        JsonParser.parseString(stdout).asJsonObject
    }.getOrNull()
}

/**
 * Root channel to the module's traeclockctl CLI. All output contracts are the
 * JSON lines documented in tc-ctl (last line of stdout is the result object).
 */
object RootCtl {

    val gson = Gson()

    init {
        Shell.enableVerboseLogging = false
        Shell.setDefaultBuilder(
            Shell.Builder.create()
                .setFlags(Shell.FLAG_REDIRECT_STDERR)
                .setTimeout(60)
        )
    }

    /** Cached module state — StatusGate re-ran root probes on every tab switch
     *  (each probe forks a su shell ≈0.5–2s, the reported "巨慢"). Cache for the
     *  process lifetime; module install state can't change without a reboot. */
    @Volatile var cachedRoot: Boolean? = null
        private set
    @Volatile var cachedModule: Boolean? = null
        private set

    fun isRootAvailable(): Boolean =
        cachedRoot ?: runCatching { Shell.getShell().isRoot }.getOrDefault(false).also { cachedRoot = it }

    private fun ctlPath(): String = "/data/adb/modules/traeclock/bin/traeclockctl"

    fun isModuleInstalled(): Boolean =
        cachedModule ?: runCatching {
            Shell.cmd("test -x ${ctlPath()} && echo yes").exec().out.joinToString("").contains("yes")
        }.getOrDefault(false).also { cachedModule = it }

    fun call(vararg args: String): CtlResult {
        val quoted = args.joinToString(" ") { "'${it.replace("'", "'\\''")}'" }
        val res = Shell.cmd("${ctlPath()} $quoted").exec()
        return CtlResult(res.code, res.out.joinToString("\n"))
    }

    /** Cached long-timeout shell for OAuth helpers (trae-wait / qoder-wait run
     *  for minutes). libsu's default 60s timeout killed them mid-login.
     *  Reuse one shell — spawning a fresh root shell per call costs ~1s. */
    @Volatile private var longShell: com.topjohnwu.superuser.Shell? = null

    private fun longTimeoutShell(): com.topjohnwu.superuser.Shell =
        longShell ?: Shell.Builder.create()
            .setFlags(Shell.FLAG_REDIRECT_STDERR)
            .setTimeout(420)
            .build()
            .also { longShell = it }

    fun callLong(vararg args: String): CtlResult {
        val quoted = args.joinToString(" ") { "'${it.replace("'", "'\''")}'" }
        val res = longTimeoutShell().newJob().add("${ctlPath()} $quoted").exec()
        return CtlResult(res.code, res.out.joinToString("\n"))
    }

    // ── typed wrappers ──────────────────────────────────────────────────────

    fun status(): JsonObject? = call("status").json()

    /** Real-time credits query across all enabled accounts (network on device). */
    fun credits(): JsonObject? = call("credits").json()

    /** Check-in credit changes (before/after each round), newest first. */
    fun creditDiffs(): List<JsonObject> {
        val obj = call("credit-diffs").json() ?: return emptyList()
        return obj.getAsJsonArray("diffs")?.map { it.asJsonObject } ?: emptyList()
    }

    fun accounts(app: String? = null): List<JsonObject> {
        val args = mutableListOf("accounts")
        app?.let { args.add(it) }
        val obj = call(*args.toTypedArray()).json() ?: return emptyList()
        return obj.getAsJsonArray("accounts")?.map { it.asJsonObject } ?: emptyList()
    }

    fun runNow(app: String?): CtlResult =
        if (app == null) call("run-now") else call("run-now", app)

    fun remove(app: String, uid: String): CtlResult = call("remove", app, uid)

    fun setEnabled(app: String, uid: String, enabled: Boolean): CtlResult =
        call(if (enabled) "enable" else "disable", app, uid)

    fun setConfig(patchJson: String): CtlResult = call("config", patchJson)

    fun log(lines: Int = 200): List<String> {
        val obj = call("log", lines.toString()).json() ?: return emptyList()
        return obj.getAsJsonArray("lines")?.map { it.asString } ?: emptyList()
    }

    fun traeLoginUrl(): JsonObject? = call("trae-login-url").json()

    /** Listen on 127.0.0.1:17388 for the OAuth redirect; blocks until callback. */
    fun traeWait(timeoutSecs: Int): CtlResult = callLong("trae-wait", timeoutSecs.toString())

    fun traeCallback(url: String): CtlResult = call("trae-callback", url)

    fun traeRefresh(uid: String): CtlResult = call("trae-refresh", uid)

    fun wbStart(): JsonObject? = call("wb-start").json()

    fun wbPoll(state: String): CtlResult = call("wb-poll", state)

    fun qoderWait(timeoutSecs: Int): CtlResult = callLong("qoder-wait", timeoutSecs.toString())
}
