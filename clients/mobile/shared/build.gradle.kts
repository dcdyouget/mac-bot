plugins {
    kotlin("multiplatform")
    kotlin("plugin.serialization")
    kotlin("plugin.compose")
    id("org.jetbrains.compose")
    id("com.android.kotlin.multiplatform.library")
}
kotlin {
    android {
        namespace = "bot.mac.mobile.shared"
        compileSdk { version = release(37) { minorApiLevel = 0 } }
        minSdk = 26
        androidResources.enable = true
        withHostTestBuilder {}.configure { isIncludeAndroidResources = true }
    }
    jvmToolchain(21)
    sourceSets {
        commonMain.dependencies {
            implementation(compose.runtime)
            implementation(compose.foundation)
            implementation(compose.material3)
            implementation(compose.ui)
            implementation(compose.components.resources)
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0")
            implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
            implementation("org.jetbrains.kotlinx:kotlinx-datetime:0.7.1")
            implementation("io.ktor:ktor-client-core:3.6.0")
            implementation("io.ktor:ktor-client-websockets:3.6.0")
            implementation("io.ktor:ktor-client-content-negotiation:3.6.0")
            implementation("io.ktor:ktor-serialization-kotlinx-json:3.6.0")
        }
        androidMain.dependencies { implementation("io.ktor:ktor-client-okhttp:3.6.0"); implementation("androidx.activity:activity-compose:1.12.4") }
        commonTest.dependencies {
            implementation(kotlin("test"))
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.11.0")
        }
        getByName("androidHostTest") {
            kotlin.srcDir("src/androidUnitTest/kotlin")
            resources.srcDir("src/androidUnitTest/resources")
            dependencies { implementation("junit:junit:4.13.2"); implementation("com.squareup.okhttp3:mockwebserver3:5.5.0"); implementation("org.robolectric:robolectric:4.16") }
        }
    }
}
compose.resources { publicResClass = true; packageOfResClass = "bot.mac.mobile.resources"; generateResClass = always }
tasks.register("commonTest") { dependsOn("testAndroidHostTest") }
tasks.register("androidUnitTest") { dependsOn("testAndroidHostTest") }
