package com.hydrarsw.cliplite

import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** Vídeos en la galería, carpeta Movies/ClipLite. */
object ClipStore {
    const val FOLDER = "Movies/ClipLite"

    data class Clip(val uri: Uri, val name: String, val durationMs: Long, val size: Long, val added: Long)

    fun stamp(): String = SimpleDateFormat("yyyy-MM-dd_HH-mm-ss", Locale.US).format(Date())

    private fun collection(): Uri = MediaStore.Video.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)

    /** Crea un vídeo vacío (oculto hasta terminar de escribirlo). */
    fun create(ctx: Context, name: String): Pair<Uri, ParcelFileDescriptor>? {
        val values = ContentValues().apply {
            put(MediaStore.Video.Media.DISPLAY_NAME, "$name.mp4")
            put(MediaStore.Video.Media.MIME_TYPE, "video/mp4")
            put(MediaStore.Video.Media.RELATIVE_PATH, FOLDER)
            put(MediaStore.Video.Media.IS_PENDING, 1)
        }
        val uri = ctx.contentResolver.insert(collection(), values) ?: return null
        val pfd = try {
            ctx.contentResolver.openFileDescriptor(uri, "rw")
        } catch (e: Exception) {
            null
        }
        if (pfd == null) {
            delete(ctx, uri)
            return null
        }
        return Pair(uri, pfd)
    }

    fun finish(ctx: Context, uri: Uri) {
        val values = ContentValues().apply { put(MediaStore.Video.Media.IS_PENDING, 0) }
        ctx.contentResolver.update(uri, values, null, null)
    }

    fun delete(ctx: Context, uri: Uri) {
        try {
            ctx.contentResolver.delete(uri, null, null)
        } catch (_: Exception) {
        }
    }

    fun list(ctx: Context): List<Clip> {
        val out = ArrayList<Clip>()
        val proj = arrayOf(
            MediaStore.Video.Media._ID,
            MediaStore.Video.Media.DISPLAY_NAME,
            MediaStore.Video.Media.DURATION,
            MediaStore.Video.Media.SIZE,
            MediaStore.Video.Media.DATE_ADDED,
        )
        val sel = "${MediaStore.Video.Media.RELATIVE_PATH} LIKE ? AND ${MediaStore.Video.Media.IS_PENDING} = 0"
        ctx.contentResolver.query(collection(), proj, sel, arrayOf("$FOLDER%"), "${MediaStore.Video.Media.DATE_ADDED} DESC")?.use { c ->
            while (c.moveToNext()) {
                val id = c.getLong(0)
                out.add(Clip(ContentUris.withAppendedId(collection(), id), c.getString(1) ?: "", c.getLong(2), c.getLong(3), c.getLong(4)))
            }
        }
        return out
    }
}
