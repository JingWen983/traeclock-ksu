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

// ── credits dashboard models (from `traeclockctl credits` JSON) ─────────────

/** One account's credit row parsed from the credits JSON. */
data class CreditItem(
    val app: String,
    val userId: String,
    val name: String,
    val ok: Boolean,
    val message: String,
    val balance: Double?,
    val general: Double?,
    val work: Double?,
    val used: Double?,
    val earliestExpire: Long?,
    val packs: List<CreditPack>,
)

data class CreditPack(
    val kind: String,
    val source: String,
    val remaining: Double?,
    val total: Double?,
    val expireTime: Long?,
    val expireText: String?,
)

data class CreditsSnapshot(
    val total: Double,
    val fetchedAt: String,
    val rows: List<CreditItem>,
)

fun parseCredits(o: com.google.gson.JsonObject): CreditsSnapshot {
    val rows = mutableListOf<CreditItem>()
    o.getAsJsonArray("rows")?.forEach { el ->
        val r = el.asJsonObject
        val packs = r.getAsJsonArray("packs")?.map { p ->
            val pk = p.asJsonObject
            CreditPack(
                kind = pk.get("kind")?.asString ?: "",
                source = pk.get("source")?.asString ?: "",
                remaining = pk.get("remaining")?.takeIf { !it.isJsonNull }?.asDouble,
                total = pk.get("total")?.takeIf { !it.isJsonNull }?.asDouble,
                expireTime = pk.get("expire_time")?.takeIf { !it.isJsonNull }?.asLong,
                expireText = pk.get("expire_text")?.takeIf { !it.isJsonNull }?.asString,
            )
        } ?: emptyList()
        rows.add(
            CreditItem(
                app = r.get("app")?.asString ?: "",
                userId = r.get("user_id")?.asString ?: "",
                name = r.get("name")?.asString ?: "",
                ok = r.get("ok")?.asBoolean ?: false,
                message = r.get("message")?.asString ?: "",
                balance = r.get("balance")?.takeIf { !it.isJsonNull }?.asDouble,
                general = r.get("general")?.takeIf { !it.isJsonNull }?.asDouble,
                work = r.get("work")?.takeIf { !it.isJsonNull }?.asDouble,
                used = r.get("used")?.takeIf { !it.isJsonNull }?.asDouble,
                earliestExpire = r.get("earliest_expire")?.takeIf { !it.isJsonNull }?.asLong,
                packs = packs,
            )
        )
    }
    return CreditsSnapshot(
        total = o.get("total")?.takeIf { !it.isJsonNull }?.asDouble ?: 0.0,
        fetchedAt = o.get("fetched_at")?.asString ?: "",
        rows = rows,
    )
}

fun fmtExpire(ts: Long?): String = when (ts) {
    null -> "到期未知"
    4102444800L -> "长期有效"
    else -> {
        val d = java.time.Instant.ofEpochSecond(ts).atZone(java.time.ZoneId.systemDefault())
        "%04d-%02d-%02d".format(d.year, d.monthValue, d.dayOfMonth)
    }
}
