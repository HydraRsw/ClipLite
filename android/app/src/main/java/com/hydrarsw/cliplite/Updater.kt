package com.hydrarsw.cliplite

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * Actualizaciones automáticas desde las Releases de GitHub (las mismas que la app de PC).
 * Descarga ClipLite-Android.apk, comprueba su huella SHA-256 publicada por GitHub y lo instala
 * con el instalador de Android (que pide confirmación, como siempre fuera de Google Play).
 */
object Updater {
    private const val API = "https://api.github.com/repos/HydraRsw/ClipLite/releases/latest"
    private const val ASSET = "ClipLite-Android.apk"

    data class Release(val version: String, val notes: String, val url: String, val sha256: String, val size: Long)

    fun parseVersion(v: String): List<Int> {
        val s = v.trim().removePrefix("v")
        if (s.contains("local")) return listOf(0, 0, 0)
        return s.split(".").map { it.toIntOrNull() ?: 0 }.let { it + List(maxOf(0, 3 - it.size)) { 0 } }
    }

    fun isNewer(remote: String, local: String): Boolean {
        val r = parseVersion(remote)
        val l = parseVersion(local)
        for (i in 0 until 3) {
            if (r[i] != l[i]) return r[i] > l[i]
        }
        return false
    }

    fun currentVersion(ctx: Context): String =
        ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName ?: "0"

    private fun open(url: String): HttpURLConnection {
        val c = URL(url).openConnection() as HttpURLConnection
        c.connectTimeout = 15_000
        c.readTimeout = 60_000
        c.setRequestProperty("User-Agent", "ClipLite-Android")
        c.setRequestProperty("Accept", "application/vnd.github+json")
        c.instanceFollowRedirects = true
        return c
    }

    /** Última versión publicada que incluye la app Android, o null si no hay o no se pudo consultar. */
    fun latest(): Release? {
        return try {
            val c = open(API)
            if (c.responseCode != 200) return null
            val j = JSONObject(c.inputStream.bufferedReader().readText())
            val assets = j.getJSONArray("assets")
            for (i in 0 until assets.length()) {
                val a = assets.getJSONObject(i)
                if (a.optString("name") != ASSET) continue
                val url = a.getString("browser_download_url")
                val digest = a.optString("digest").removePrefix("sha256:").lowercase()
                if (!url.startsWith("https://github.com/") || digest.length != 64) return null
                return Release(j.getString("tag_name").removePrefix("v"), j.optString("body"), url, digest, a.optLong("size"))
            }
            null
        } catch (e: Exception) {
            Log.w(Engine.TAG, "Actualizaciones: $e")
            null
        }
    }

    fun sha256(f: File): String {
        val md = MessageDigest.getInstance("SHA-256")
        f.inputStream().use { inp ->
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = inp.read(buf)
                if (n <= 0) break
                md.update(buf, 0, n)
            }
        }
        return md.digest().joinToString("") { "%02x".format(it) }
    }

    /** Descarga y verifica. Devuelve el archivo listo para instalar. */
    fun download(ctx: Context, r: Release, progress: (Int) -> Unit): File {
        val f = File(ctx.cacheDir, "actualizacion.apk")
        val c = open(r.url)
        if (c.responseCode != 200) throw Exception("GitHub respondió ${c.responseCode}")
        val total = if (r.size > 0) r.size else c.contentLengthLong
        c.inputStream.use { inp ->
            f.outputStream().use { out ->
                val buf = ByteArray(64 * 1024)
                var done = 0L
                while (true) {
                    val n = inp.read(buf)
                    if (n <= 0) break
                    out.write(buf, 0, n)
                    done += n
                    if (total > 0) progress((done * 100 / total).toInt())
                }
            }
        }
        if (r.size > 0 && f.length() != r.size) throw Exception("descarga incompleta")
        if (sha256(f) != r.sha256) {
            f.delete()
            throw Exception("la huella SHA-256 no coincide; se descarta")
        }
        return f
    }

    /** Abre el instalador de Android con la versión descargada. */
    fun install(ctx: Context, apk: File) {
        val installer = ctx.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        params.setAppPackageName(ctx.packageName)
        val id = installer.createSession(params)
        installer.openSession(id).use { session ->
            session.openWrite("ClipLite.apk", 0, apk.length()).use { out ->
                apk.inputStream().use { it.copyTo(out) }
                session.fsync(out)
            }
            val intent = Intent(ctx, InstallReceiver::class.java)
            // El sistema añade datos al aviso: en Android 12+ debe ser modificable.
            val mutable = if (android.os.Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
            val pi = PendingIntent.getBroadcast(ctx, id, intent, PendingIntent.FLAG_UPDATE_CURRENT or mutable)
            session.commit(pi.intentSender)
        }
    }
}
