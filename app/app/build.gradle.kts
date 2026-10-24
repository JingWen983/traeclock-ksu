plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "cn.traeclock.manager"
    compileSdk = 37

    defaultConfig {
        applicationId = "cn.traeclock.manager"
        minSdk = 26
        targetSdk = 37
        versionCode = 16
        versionName = "0.5.2"
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
    val composeVersion = "1.10.3"
    implementation("androidx.compose.ui:ui:$composeVersion")
    implementation("androidx.compose.ui:ui-tooling-preview:$composeVersion")
    implementation("androidx.compose.foundation:foundation:$composeVersion")
    // Liquid Glass bottom bar (Kyant0/AndroidLiquidGlass)
    implementation("io.github.kyant0:backdrop:1.0.6")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.6")
    implementation("androidx.core:core-ktx:1.13.1")
    // Miuix (HyperOS design) — replaces the Material chrome layer (stable 0.9.4)
    implementation("top.yukonga.miuix.kmp:miuix-ui-android:0.9.4")
    implementation("top.yukonga.miuix.kmp:miuix-preference-android:0.9.4")
    implementation("com.github.topjohnwu.libsu:core:5.2.2")
    implementation("com.github.topjohnwu.libsu:service:5.2.2")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    implementation("com.google.code.gson:gson:2.11.0")
    debugImplementation("androidx.compose.ui:ui-tooling:$composeVersion")
}
