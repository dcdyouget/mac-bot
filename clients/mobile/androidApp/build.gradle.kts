plugins { id("com.android.application"); kotlin("plugin.compose") }
android {
    namespace = "bot.mac.mobile"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    defaultConfig { applicationId = "bot.mac.mobile"; minSdk = 26; targetSdk = 36; versionCode = 1; versionName = "0.1.0"; testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner" }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_21; targetCompatibility = JavaVersion.VERSION_21 }
    buildFeatures { compose = true }
    testBuildType = providers.gradleProperty("macbotTestBuildType").orElse("debug").get()
    val releaseStore = providers.environmentVariable("MACBOT_ANDROID_KEYSTORE").orNull
    signingConfigs {
        if (releaseStore != null) create("distribution") {
            storeFile = file(releaseStore)
            storePassword = providers.environmentVariable("MACBOT_ANDROID_STORE_PASSWORD").get()
            keyAlias = providers.environmentVariable("MACBOT_ANDROID_KEY_ALIAS").get()
            keyPassword = providers.environmentVariable("MACBOT_ANDROID_KEY_PASSWORD").get()
        }
    }
    buildTypes { release { if (releaseStore != null) signingConfig = signingConfigs.getByName("distribution"); isShrinkResources = true; isMinifyEnabled = true; proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro"); testProguardFiles("proguard-test-rules.pro") } }
}
dependencies {
    implementation(project(":shared"))
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("com.google.errorprone:error_prone_annotations:2.36.0")
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.core:core-ktx:1.17.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
}
