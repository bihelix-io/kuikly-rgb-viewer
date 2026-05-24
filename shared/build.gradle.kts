plugins {
    kotlin("multiplatform")
    id("com.android.library")
    id("com.google.devtools.ksp")
    id("com.tencent.kuikly-open.kuikly")
}

val kuiklyVersion: String by project
val ktorVersion: String by project
val coroutinesVersion: String by project
val serializationJsonVersion: String by project

kotlin {
    androidTarget()

    sourceSets {
        val commonMain by getting {
            dependencies {
                implementation("com.tencent.kuikly-open:core:$kuiklyVersion")
                implementation("com.tencent.kuikly-open:compose:$kuiklyVersion")
                implementation("com.tencent.kuikly-open:core-annotations:$kuiklyVersion")
                implementation("io.ktor:ktor-client-core:$ktorVersion")
                implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:$coroutinesVersion")
                implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:$serializationJsonVersion")
            }
        }
        val commonTest by getting {
            dependencies {
                implementation(kotlin("test"))
            }
        }
        val androidMain by getting {
            dependencies {
                implementation("io.ktor:ktor-client-okhttp:$ktorVersion")
            }
        }
    }
}

ksp {
    arg("pageName", project.properties["pageName"] as? String ?: "")
}

dependencies {
    add("kspAndroid", "com.tencent.kuikly-open:core-ksp:$kuiklyVersion")
}

android {
    namespace = "io.bihelix.rgbviewer"
    compileSdk = 34

    sourceSets["main"].manifest.srcFile("src/androidMain/AndroidManifest.xml")

    defaultConfig {
        minSdk = 21
        targetSdk = 34
    }
}
