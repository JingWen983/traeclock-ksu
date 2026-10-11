package cn.traeclock.manager

/** App id constants mirroring the module store. */
object Apps {
    const val TRAE = "trae"
    const val WB = "wb"
    const val QODER = "qoder"
    val ALL = listOf(TRAE, WB, QODER)

    fun displayName(app: String) = when (app) {
        TRAE -> "Trae"
        WB -> "WorkBuddy"
        QODER -> "Qoder"
        else -> app
    }
}

/** Account view model parsed from `traeclockctl accounts` JSON. */
data class AccountRow(
    val app: String,
    val userId: String,
    val name: String,
    val enabled: Boolean,
    val needsRelogin: Boolean,
    val reloginReason: String,
    val jwtStatus: String,
    val jwtExpHours: Double?,
    val deviceId: String?,
)

fun parseAccount(o: com.google.gson.JsonObject): AccountRow = AccountRow(
    app = o.get("app")?.asString.orEmpty(),
    userId = o.get("user_id")?.asString.orEmpty(),
    name = o.get("name")?.asString.orEmpty(),
    enabled = o.get("enabled")?.asBoolean ?: false,
    needsRelogin = o.get("needs_relogin")?.asBoolean ?: false,
    reloginReason = o.get("relogin_reason")?.asString.orEmpty(),
    jwtStatus = o.get("jwt_status")?.asString.orEmpty(),
    jwtExpHours = o.get("jwt_exp_hours")?.takeIf { !it.isJsonNull }?.asDouble,
    deviceId = o.get("device_id")?.takeIf { !it.isJsonNull }?.asString,
)

/** Status view model parsed from `traeclockctl status` JSON. */
data class StatusView(
    val daemonPid: Long?,
    val platformFlavor: String,
    val jitter: Boolean,
    val windowStart: Int,
    val windowEnd: Int,
    val accountGap: Long,
    val counts: Map<String, CountRow>,
    val nextApp: String?,
    val nextInSecs: Long?,
)

data class CountRow(val total: Int, val enabled: Int, val needsRelogin: Int)

fun parseStatus(o: com.google.gson.JsonObject): StatusView {
    val cfg = o.getAsJsonObject("config")
    val counts = mutableMapOf<String, CountRow>()
    o.getAsJsonObject("counts")?.entrySet()?.forEach { (app, v) ->
        val c = v.asJsonObject
        counts[app] = CountRow(
            total = c.get("total")?.asInt ?: 0,
            enabled = c.get("enabled")?.asInt ?: 0,
            needsRelogin = c.get("needs_relogin")?.asInt ?: 0,
        )
    }
    val next = o.getAsJsonObject("next")
    return StatusView(
        daemonPid = o.get("daemon_pid")?.takeIf { !it.isJsonNull }?.asLong,
        platformFlavor = cfg?.get("platform_flavor")?.asString ?: "android",
        jitter = cfg?.get("jitter")?.asBoolean ?: true,
        windowStart = cfg?.get("window_start_min")?.asInt ?: 480,
        windowEnd = cfg?.get("window_end_min")?.asInt ?: 600,
        accountGap = cfg?.get("account_gap_secs")?.asLong ?: 3,
        counts = counts,
        nextApp = next?.get("app")?.asString,
        nextInSecs = next?.get("in_secs")?.takeIf { !it.isJsonNull }?.asLong,
    )
}

fun formatDuration(secs: Long): String = when {
    secs <= 0 -> "现在"
    secs < 3600 -> "${secs / 60} 分钟后"
    secs < 86400 -> "${"%.1f".format(secs / 3600.0)} 小时后"
    else -> "${"%.1f".format(secs / 86400.0)} 天后"
}
