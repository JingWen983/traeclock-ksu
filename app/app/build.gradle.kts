plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// 版本号唯一来源：仓库根的 VERSION（scripts/bump-version.sh 维护）。
// gradle 直接从文件派生，所以 APK 的 versionName/versionCode 不会和 VERSION 漂移；
// 改版本只需跑脚本（它同时同步 daemon/Cargo.toml 与 module/module.prop）。
val appVersionName: String = rootProject.file("../VERSION").readText().trim()
val appVersionCode: Int = appVersionName.split(".").let { parts ->
    require(parts.size == 3) { "VERSION 必须是 X.Y.Z，实际: $appVersionName" }
    val (major, minor, patch) = parts.map { it.trim().toInt() }
    require(major < 100 && minor < 100 && patch < 100) {
        "VERSION 每段必须 < 100（versionCode 段位编码），实际: $appVersionName"
    }
    major * 10_000 + minor * 100 + patch
}

android {
    namespace = "cn.traeclock.manager"
    compileSdk = 37

    defaultConfig {
        applicationId = "cn.traeclock.manager"
        minSdk = 26
        targetSdk = 37
        versionCode = appVersionCode
        versionName = appVersionName
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
    }
}

dependencies {
    // miuix 0.9.4's Window/Overlay popups use androidx.navigationevent's
    // NavigationBackHandler, whose dispatcher owner is installed by
    // ComponentActivity >= 1.13 (older 1.9.2 left it unset → sheet open crashed
    // with "No NavigationEventDispatcher was provided")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.activity:activity-ktx:1.13.0")
    implementation("androidx.navigationevent:navigationevent-compose:1.1.2")
    // pinned (no BOM): backdrop 1.0.6 is compiled against compose-ui 1.10.3 and
    // needs a runtime >= that; the old BOM 2024.09.02 pinned ui 1.7.x which
    // broke its GraphicsLayer-based backdrop sampling
    val composeVersion = "1.12.1"
    implementation("androidx.compose.ui:ui:$composeVersion")
    implementation("androidx.compose.ui:ui-tooling-preview:$composeVersion")
    implementation("androidx.compose.foundation:foundation:$composeVersion")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.6")
    implementation("androidx.core:core-ktx:1.13.1")
    // Miuix (HyperOS design) — replaces the Material chrome layer (stable 0.9.4)
    implementation("top.yukonga.miuix.kmp:miuix-ui-android:0.9.4")
    implementation("top.yukonga.miuix.kmp:miuix-preference-android:0.9.4")
    // HyperNavBar-style chrome: extended icons + blur + Haze progressive top bar
    implementation("top.yukonga.miuix.kmp:miuix-icons-android:0.9.4")
    implementation("top.yukonga.miuix.kmp:miuix-blur-android:0.9.4")
    implementation("dev.chrisbanes.haze:haze:1.7.3")
    implementation("com.github.topjohnwu.libsu:core:5.2.2")
    implementation("com.github.topjohnwu.libsu:service:5.2.2")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    implementation("com.google.code.gson:gson:2.11.0")
    debugImplementation("androidx.compose.ui:ui-tooling:$composeVersion")
}
