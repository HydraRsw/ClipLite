package com.hydrarsw.cliplite

import android.annotation.SuppressLint
import android.content.Context
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.MediaMuxer
import android.media.MediaRecorder
import android.media.projection.MediaProjection
import android.net.Uri
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Un trozo ya codificado (vídeo = pista 0, audio = pista 1). */
class Sample(val track: Int, val data: ByteArray, val ptsUs: Long, val flags: Int) {
    val isKey get() = flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
}

/**
 * Motor de grabación del móvil.
 *  - Pantalla → VirtualDisplay → codificador H.264 por hardware (la imagen no pasa por la CPU).
 *  - Sonido interno del juego (AudioPlaybackCapture, Android 10+) y, si se quiere, micrófono → AAC.
 *  - Los trozos codificados se guardan en memoria los últimos N segundos (búfer de clips).
 *    Guardar un clip = copiar esos trozos a un MP4, sin volver a codificar (instantáneo).
 *  - En modo "partida completa" además se escribe todo a un MP4 mientras se juega.
 */
class Engine(
    private val ctx: Context,
    private val projection: MediaProjection,
    private val cfg: Prefs.Config,
) {
    companion object {
        const val TAG = "ClipLite"
        private const val RATE = 48_000
    }

    private val lock = Object()
    private val ring = ArrayDeque<Sample>()
    @Volatile private var running = false
    @Volatile var videoFormat: MediaFormat? = null
    @Volatile var audioFormat: MediaFormat? = null
    @Volatile var audioEnabled = false
        private set

    private var vCodec: MediaCodec? = null
    private var aCodec: MediaCodec? = null
    private var display: VirtualDisplay? = null
    private var gameRec: AudioRecord? = null
    private var micRec: AudioRecord? = null
    private var vThread: Thread? = null
    private var aThread: Thread? = null
    private var full: FullWriter? = null

    var width = 0
        private set
    var height = 0
        private set

    // ------------------------------------------------------------------ inicio

    fun start(screenW: Int, screenH: Int, dpi: Int) {
        running = true
        startVideo(screenW, screenH, dpi)
        startAudio()
        if (cfg.fullMatch) {
            full = FullWriter(ctx, audioEnabled)
        }
    }

    private fun even16(x: Double) = maxOf(16, (Math.round(x / 16.0) * 16).toInt())

    /** Tamaño de salida: lado corto 720/1080 y lado largo como máximo 1920 (compatible con todos los móviles). */
    private fun outSize(w: Int, h: Int, short: Int): Pair<Int, Int> {
        val minDim = minOf(w, h).toDouble()
        val maxDim = maxOf(w, h).toDouble()
        val scale = minOf(short / minDim, 1920.0 / maxDim, 1.0)
        return Pair(even16(w * scale), even16(h * scale))
    }

    private fun startVideo(screenW: Int, screenH: Int, dpi: Int) {
        var lastError: Exception? = null
        // Si el codificador no acepta el tamaño, se prueba uno menor.
        for (short in listOf(cfg.shortSide, 720, 480).distinct()) {
            val (w, h) = outSize(screenW, screenH, short)
            val bitrate = if (short >= 1080) cfg.bitrateHigh else if (short >= 720) cfg.bitrateLow else 3_000_000
            try {
                val f = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h).apply {
                    setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                    setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
                    setInteger(MediaFormat.KEY_FRAME_RATE, cfg.fps)
                    setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
                    // Si la pantalla no cambia, repetir el fotograma para que el vídeo no se "congele".
                    setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 1_000_000L / cfg.fps)
                    setInteger(MediaFormat.KEY_PRIORITY, 0) // tiempo real
                }
                val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
                try {
                    codec.configure(f, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                } catch (e: Exception) {
                    codec.release()
                    throw e
                }
                val surface = codec.createInputSurface()
                codec.start()
                vCodec = codec
                width = w
                height = h
                display = projection.createVirtualDisplay(
                    "ClipLite", w, h, dpi,
                    DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, null,
                )
                Log.i(TAG, "Vídeo ${w}x$h a ${cfg.fps} fps, ${bitrate / 1000} kbps")
                vThread = Thread({ drainVideo(codec) }, "video").apply { start() }
                return
            } catch (e: Exception) {
                Log.w(TAG, "El codificador no aceptó ${w}x$h: $e")
                lastError = e
            }
        }
        throw lastError ?: IllegalStateException("sin codificador de vídeo")
    }

    private fun drainVideo(codec: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        while (true) {
            val idx = try { codec.dequeueOutputBuffer(info, 10_000) } catch (e: Exception) { break }
            when {
                idx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> videoFormat = codec.outputFormat
                idx >= 0 -> {
                    val eos = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0 && info.size > 0) {
                        val buf = codec.getOutputBuffer(idx)!!
                        buf.position(info.offset)
                        buf.limit(info.offset + info.size)
                        val bytes = ByteArray(info.size)
                        buf.get(bytes)
                        push(Sample(0, bytes, info.presentationTimeUs, info.flags))
                    }
                    codec.releaseOutputBuffer(idx, false)
                    if (eos) break
                }
            }
        }
    }

    @SuppressLint("MissingPermission") // se pide antes de empezar
    private fun startAudio() {
        if (!cfg.audio) return
        try {
            val fmt = AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(RATE)
                .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                .build()
            val capture = AudioPlaybackCaptureConfiguration.Builder(projection)
                .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                .addMatchingUsage(AudioAttributes.USAGE_GAME)
                .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                .build()
            val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT)
            val rec = AudioRecord.Builder()
                .setAudioFormat(fmt)
                .setBufferSizeInBytes(maxOf(min * 4, RATE * 4 / 2))
                .setAudioPlaybackCaptureConfig(capture)
                .build()
            if (rec.state != AudioRecord.STATE_INITIALIZED) {
                rec.release()
                Log.w(TAG, "No se pudo abrir el sonido interno")
                return
            }
            gameRec = rec
            if (cfg.mic) {
                val mmin = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
                val m = AudioRecord(MediaRecorder.AudioSource.MIC, RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(mmin * 4, RATE))
                if (m.state == AudioRecord.STATE_INITIALIZED) micRec = m else m.release()
            }
            val af = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_AAC, RATE, 2).apply {
                setInteger(MediaFormat.KEY_AAC_PROFILE, MediaCodecInfo.CodecProfileLevel.AACObjectLC)
                setInteger(MediaFormat.KEY_BIT_RATE, 160_000)
                setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 16_384)
            }
            val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_AAC)
            codec.configure(af, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            codec.start()
            aCodec = codec
            audioEnabled = true
            rec.startRecording()
            micRec?.startRecording()
            aThread = Thread({ audioLoop(rec, micRec, codec) }, "audio").apply { start() }
        } catch (e: Exception) {
            Log.w(TAG, "Audio no disponible: $e")
            audioEnabled = false
        }
    }

    private fun audioLoop(rec: AudioRecord, mic: AudioRecord?, codec: MediaCodec) {
        val frames = 1024 // ~21 ms
        val game = ShortArray(frames * 2)
        val micBuf = ShortArray(frames)
        val mixed = ByteBuffer.allocate(frames * 4).order(ByteOrder.LITTLE_ENDIAN)
        val info = MediaCodec.BufferInfo()
        var baseUs = -1L
        var total = 0L
        while (running) {
            val n = rec.read(game, 0, game.size) // bloquea hasta tener audio (siempre llega, aunque sea silencio)
            if (n <= 0) continue
            if (baseUs < 0) baseUs = System.nanoTime() / 1000 - (n / 2) * 1_000_000L / RATE
            val got = n / 2
            var m = 0
            if (mic != null) {
                m = mic.read(micBuf, 0, got, AudioRecord.READ_NON_BLOCKING).coerceAtLeast(0)
            }
            mixed.clear()
            for (i in 0 until got) {
                val v = if (i < m) micBuf[i].toInt() else 0
                mixed.putShort((game[2 * i] + v).coerceIn(-32768, 32767).toShort())
                mixed.putShort((game[2 * i + 1] + v).coerceIn(-32768, 32767).toShort())
            }
            mixed.flip()
            var pts = baseUs + total * 1_000_000L / RATE
            // Si Android dejó de entregar sonido un rato (pausa, llamada…), re-sincronizar con el reloj
            // para que el audio no quede adelantado respecto al vídeo.
            val nowStart = System.nanoTime() / 1000 - got * 1_000_000L / RATE
            if (nowStart - pts > 200_000) {
                baseUs += nowStart - pts
                pts = nowStart
            }
            total += got
            val inIdx = codec.dequeueInputBuffer(10_000)
            if (inIdx >= 0) {
                val inBuf = codec.getInputBuffer(inIdx)!!
                inBuf.clear()
                inBuf.put(mixed)
                codec.queueInputBuffer(inIdx, 0, got * 4, pts, 0)
            }
            drainAudio(codec, info)
        }
    }

    private fun drainAudio(codec: MediaCodec, info: MediaCodec.BufferInfo) {
        while (true) {
            val idx = codec.dequeueOutputBuffer(info, 0)
            if (idx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                audioFormat = codec.outputFormat
                continue
            }
            if (idx < 0) return
            if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0 && info.size > 0) {
                val buf = codec.getOutputBuffer(idx)!!
                buf.position(info.offset)
                buf.limit(info.offset + info.size)
                val bytes = ByteArray(info.size)
                buf.get(bytes)
                push(Sample(1, bytes, info.presentationTimeUs, info.flags))
            }
            codec.releaseOutputBuffer(idx, false)
        }
    }

    // ------------------------------------------------------------------ búfer de clips

    private fun push(s: Sample) {
        synchronized(lock) {
            ring.addLast(s)
            // Se guardan los segundos del clip + margen (para encontrar un fotograma clave antes).
            val cutoff = s.ptsUs - (cfg.clipSeconds + 6) * 1_000_000L
            while (ring.isNotEmpty() && ring.first().ptsUs < cutoff) ring.removeFirst()
        }
        full?.write(s, videoFormat, audioFormat)
    }

    /** Guarda los últimos [seconds] segundos en la galería. Devuelve la dirección o null. */
    fun saveClip(seconds: Int, label: String): Uri? {
        val vf = videoFormat ?: return null
        val af = audioFormat
        val snap = synchronized(lock) { ring.toList() }
        val video = snap.filter { it.track == 0 }
        if (video.isEmpty()) return null
        val end = video.last().ptsUs
        val want = end - seconds * 1_000_000L
        // Empezar en el último fotograma clave anterior al inicio pedido (o el primero que haya).
        val key = video.lastOrNull { it.isKey && it.ptsUs <= want } ?: video.firstOrNull { it.isKey } ?: return null
        val start = key.ptsUs
        val (uri, pfd) = ClipStore.create(ctx, "ClipLite_${ClipStore.stamp()}_$label") ?: return null
        try {
            val mux = MediaMuxer(pfd.fileDescriptor, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
            val vt = mux.addTrack(vf)
            val at = if (af != null && audioEnabled) mux.addTrack(af) else -1
            mux.start()
            val info = MediaCodec.BufferInfo()
            for (s in snap) {
                if (s.ptsUs < start || s.ptsUs > end) continue
                val track = if (s.track == 0) vt else at
                if (track < 0) continue
                info.set(0, s.data.size, s.ptsUs - start, if (s.track == 0) s.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
                mux.writeSampleData(track, ByteBuffer.wrap(s.data), info)
            }
            mux.stop()
            mux.release()
            pfd.close()
            ClipStore.finish(ctx, uri)
            Log.i(TAG, "Clip guardado: $uri (${(end - start) / 1000} ms)")
            return uri
        } catch (e: Exception) {
            Log.e(TAG, "Error al guardar el clip", e)
            try { pfd.close() } catch (_: Exception) {}
            ClipStore.delete(ctx, uri)
            return null
        }
    }

    // ------------------------------------------------------------------ parada

    /** Detiene todo. Devuelve la partida completa guardada (si se grababa). */
    fun stop(): Uri? {
        if (!running) return null
        running = false
        try { vCodec?.signalEndOfInputStream() } catch (_: Exception) {}
        vThread?.join(3000)
        aThread?.join(3000)
        try { display?.release() } catch (_: Exception) {}
        for (r in listOf(gameRec, micRec)) {
            try { r?.stop() } catch (_: Exception) {}
            r?.release()
        }
        for (c in listOf(vCodec, aCodec)) {
            try { c?.stop() } catch (_: Exception) {}
            try { c?.release() } catch (_: Exception) {}
        }
        return full?.finish()
    }
}

/** Escribe la partida completa a un MP4 mientras se juega. */
class FullWriter(private val ctx: Context, private val withAudio: Boolean) {
    private val lock = Object()
    private var uri: Uri? = null
    private var pfd: android.os.ParcelFileDescriptor? = null
    private var mux: MediaMuxer? = null
    private var vt = -1
    private var at = -1
    private var base = -1L
    private val pending = ArrayList<Sample>()
    private val info = MediaCodec.BufferInfo()
    private val createdAt = System.nanoTime()

    fun write(s: Sample, vf: MediaFormat?, af: MediaFormat?) {
        synchronized(lock) { writeLocked(s, vf, af) }
    }

    private fun writeLocked(s: Sample, vf: MediaFormat?, af: MediaFormat?) {
        if (mux == null) {
            // Esperar a conocer los formatos (el audio, como mucho 3 s).
            val audioReady = !withAudio || af != null || System.nanoTime() - createdAt > 3_000_000_000L
            if (vf == null || !audioReady) {
                if (pending.size < 2000) pending.add(s)
                return
            }
            val (u, p) = ClipStore.create(ctx, "ClipLite_${ClipStore.stamp()}_partida") ?: return
            uri = u
            pfd = p
            val m = MediaMuxer(p.fileDescriptor, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
            vt = m.addTrack(vf)
            if (withAudio && af != null) at = m.addTrack(af)
            m.start()
            mux = m
            val queued = ArrayList(pending)
            pending.clear()
            for (q in queued) writeNow(q)
        }
        writeNow(s)
    }

    private fun writeNow(s: Sample) {
        val m = mux ?: return
        if (base < 0) {
            if (s.track != 0 || !s.isKey) return // empezar en un fotograma clave
            base = s.ptsUs
        }
        if (s.ptsUs < base) return
        val track = if (s.track == 0) vt else at
        if (track < 0) return
        info.set(0, s.data.size, s.ptsUs - base, if (s.track == 0) s.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
        try {
            m.writeSampleData(track, ByteBuffer.wrap(s.data), info)
        } catch (e: Exception) {
            Log.w(Engine.TAG, "Partida: trozo descartado: $e")
        }
    }

    fun finish(): Uri? {
        synchronized(lock) { return finishLocked() }
    }

    private fun finishLocked(): Uri? {
        val m = mux
        val u = uri
        if (m == null || u == null) return null
        return try {
            m.stop()
            m.release()
            pfd?.close()
            ClipStore.finish(ctx, u)
            u
        } catch (e: Exception) {
            Log.e(Engine.TAG, "Error al cerrar la partida", e)
            try { pfd?.close() } catch (_: Exception) {}
            ClipStore.delete(ctx, u)
            null
        }
    }
}
