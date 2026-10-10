plugins { id("com.android.application"); kotlin("plugin.compose"); kotlin("plugin.serialization") }
val notificationCapacityIsolated = providers.gradleProperty("macbotNotificationCapacityIsolated").orNull == "true"
android {
    namespace = "bot.mac.mobile"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig {
        applicationId = if (notificationCapacityIsolated) "bot.mac.mobile.capacitytest" else "bot.mac.mobile"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_21; targetCompatibility = JavaVersion.VERSION_21 }
    buildFeatures { compose = true }
    testOptions { unitTests.isIncludeAndroidResources = true }
    val releaseStore = providers.environmentVariable("MACBOT_ANDROID_KEYSTORE").orNull
    signingConfigs {
        if (releaseStore != null) create("distribution") {
            storeFile = file(releaseStore)
            storePassword = providers.environmentVariable("MACBOT_ANDROID_STORE_PASSWORD").get()
            keyAlias = providers.environmentVariable("MACBOT_ANDROID_KEY_ALIAS").get()
            keyPassword = providers.environmentVariable("MACBOT_ANDROID_KEY_PASSWORD").get()
        }
    }
    buildTypes {
        debug {
            if (providers.gradleProperty("macbotSignedDeviceTests").orNull == "true") {
                require(releaseStore != null) { "Signed device tests require the distribution signing environment" }
                signingConfig = signingConfigs.getByName("distribution")
            }
        }
        release { if (releaseStore != null) signingConfig = signingConfigs.getByName("distribution"); isShrinkResources = true; isMinifyEnabled = true; proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro") }
    }
}
dependencies {
    implementation(project(":shared"))
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:rules:1.7.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.16")
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.core:core-ktx:1.17.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
}
