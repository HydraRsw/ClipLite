// App Android de ClipLite: graba lo que juegas en el móvil y guarda clips.
// Sin librerías externas: solo el SDK de Android (APK pequeño).
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// La versión la fija GitHub Actions (igual que la de PC): 1.0.<número de compilación>.
val versionNameProp = (project.findProperty("versionName") as String?) ?: "1.0.0-local"
val versionCodeProp = ((project.findProperty("versionCode") as String?) ?: "1").toInt()

android {
    namespace = "com.hydrarsw.cliplite"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.hydrarsw.cliplite"
        minSdk = 29 // Android 10: necesario para capturar el sonido interno de los juegos
        targetSdk = 34
        versionCode = versionCodeProp
        versionName = versionNameProp
    }

    signingConfigs {
        create("release") {
            // La clave llega desde un secreto de GitHub (nunca está en el repositorio).
            storeFile = file("../clave.jks")
            storePassword = "cliplite-android"
            keyAlias = "cliplite"
            keyPassword = "cliplite-android"
            storeType = "PKCS12"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            signingConfig = signingConfigs.getByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    lint {
        // Esta app no se publica en Google Play: los avisos de lint no deben frenar la compilación.
        checkReleaseBuilds = false
        abortOnError = false
    }
}
