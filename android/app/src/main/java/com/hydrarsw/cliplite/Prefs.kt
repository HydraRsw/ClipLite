package com.hydrarsw.cliplite

import android.content.Context

/** Ajustes de la app (se guardan en el teléfono). */
object Prefs {
    data class Config(
        val fullMatch: Boolean,
        val clipSeconds: Int,
        val shortSide: Int,
        val fps: Int,
        val audio: Boolean,
        val mic: Boolean,
        val bubble: Boolean,
    ) {
        val bitrateHigh get() = if (fps > 30) 14_000_000 else 10_000_000
        val bitrateLow get() = if (fps > 30) 8_000_000 else 6_000_000
    }

    private fun sp(ctx: Context) = ctx.getSharedPreferences("ajustes", Context.MODE_PRIVATE)

    fun load(ctx: Context): Config {
        val p = sp(ctx)
        return Config(
            fullMatch = p.getBoolean("partida", false),
            clipSeconds = p.getInt("segundos", 30),
            shortSide = p.getInt("calidad", 1080),
            fps = p.getInt("fps", 60),
            audio = p.getBoolean("audio", true),
            mic = p.getBoolean("mic", false),
            bubble = p.getBoolean("burbuja", true),
        )
    }

    fun set(ctx: Context, key: String, value: Any) {
        val e = sp(ctx).edit()
        when (value) {
            is Boolean -> e.putBoolean(key, value)
            is Int -> e.putInt(key, value)
            is String -> e.putString(key, value)
        }
        e.apply()
    }

    fun getString(ctx: Context, key: String): String? = sp(ctx).getString(key, null)
}
