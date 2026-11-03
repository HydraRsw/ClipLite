package com.hydrarsw.cliplite

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.Icon
import android.hardware.display.DisplayManager
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.Settings
import android.util.DisplayMetrics
import android.view.Display
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.widget.TextView
import android.widget.Toast
import java.util.concurrent.Executors

/**
 * Servicio que graba mientras juegas (Android exige que sea un servicio visible con notificación).
 * Controles: botón flotante (toque = guardar clip, mantener = detener) y botones de la notificación.
 */
class RecorderService : Service() {
    companion object {
        const val ACTION_START = "com.hydrarsw.cliplite.START"
        const val ACTION_CLIP = "com.hydrarsw.cliplite.CLIP"
        const val ACTION_STOP = "com.hydrarsw.cliplite.STOP"
        const val EXTRA_CODE = "code"
        const val EXTRA_DATA = "data"
        private const val CHANNEL = "grabacion"
        private const val NOTIF_ID = 1

        @Volatile var isRunning = false
            private set
        /** La pantalla principal se entera de los cambios (empezar, parar, clip guardado). */
        @Volatile var onChange: (() -> Unit)? = null
    }

    private var engine: Engine? = null
    private var projection: MediaProjection? = null
    private var cfg: Prefs.Config? = null
    private val main = Handler(Looper.getMainLooper())
    private val bg = Executors.newSingleThreadExecutor()
    private var bubble: View? = null
    private var saving = false
    private var clips = 0

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> start(intent)
            ACTION_CLIP -> clip()
            ACTION_STOP -> stopRecording()
        }
        return START_NOT_STICKY
    }

    private fun toast(msg: String) = main.post { Toast.makeText(this, msg, Toast.LENGTH_SHORT).show() }

    // ------------------------------------------------------------------ empezar

    private fun start(intent: Intent) {
        if (isRunning) return
        createChannel()
        // Android 14+: primero el servicio visible, después el permiso de captura.
        startForeground(NOTIF_ID, notification("Preparando…"), ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        val code = intent.getIntExtra(EXTRA_CODE, 0)
        val data: Intent? = if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(EXTRA_DATA, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(EXTRA_DATA)
        }
        val mpm = getSystemService(MediaProjectionManager::class.java)
        val p = try {
            if (data != null) mpm.getMediaProjection(code, data) else null
        } catch (e: Exception) {
            null
        }
        if (p == null) {
            toast("No se pudo empezar a grabar la pantalla")
            finishService()
            return
        }
        projection = p
        p.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                main.post { stopRecording() }
            }
        }, main)

        val c = Prefs.load(this)
        cfg = c
        val dm = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val m = DisplayMetrics()
        @Suppress("DEPRECATION")
        dm.getRealMetrics(m)
        // Los juegos suelen ir en horizontal: si el móvil está en vertical al empezar, grabar en horizontal.
        val (w, h) = if (m.heightPixels > m.widthPixels) Pair(m.heightPixels, m.widthPixels) else Pair(m.widthPixels, m.heightPixels)
        val e = Engine(this, p, c)
        try {
            e.start(w, h, m.densityDpi)
        } catch (ex: Exception) {
            toast("Tu móvil no pudo iniciar la grabación: ${ex.message}")
            e.stop()
            p.stop()
            finishService()
            return
        }
        engine = e
        isRunning = true
        clips = 0
        if (c.bubble && Settings.canDrawOverlays(this)) showBubble()
        val audioNote = if (c.audio && !e.audioEnabled) " (sin sonido)" else ""
        updateNotification("Grabando ${e.width}x${e.height}$audioNote · toca «Guardar clip»")
        onChange?.invoke()
    }

    // ------------------------------------------------------------------ clips

    private fun clip() {
        val e = engine ?: return
        val c = cfg ?: return
        if (saving) {
            toast("Ya se está guardando un clip")
            return
        }
        saving = true
        flashBubble()
        toast("Guardando clip…")
        // Se esperan 3 s para incluir lo que pasa justo después de pulsar.
        main.postDelayed({
            bg.execute {
                val uri = e.saveClip(c.clipSeconds + 3, "clip")
                main.post {
                    saving = false
                    if (uri != null) {
                        clips++
                        toast("✓ Clip guardado en la galería")
                        updateNotification("Grabando · $clips clip${if (clips == 1) "" else "s"} guardado${if (clips == 1) "" else "s"}")
                    } else {
                        toast("No se pudo guardar el clip")
                    }
                    onChange?.invoke()
                }
            }
        }, 3000)
    }

    // ------------------------------------------------------------------ parar

    private fun stopRecording() {
        if (!isRunning) {
            finishService()
            return
        }
        isRunning = false
        hideBubble()
        updateNotification("Guardando…")
        val e = engine
        val full = cfg?.fullMatch == true
        engine = null
        bg.execute {
            val uri = e?.stop()
            try { projection?.stop() } catch (_: Exception) {}
            projection = null
            main.post {
                toast(if (full && uri != null) "✓ Partida guardada en la galería" else "Grabación detenida")
                finishService()
                onChange?.invoke()
            }
        }
    }

    private fun finishService() {
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        hideBubble()
        super.onDestroy()
    }

    // ------------------------------------------------------------------ notificación

    private fun createChannel() {
        val nm = getSystemService(NotificationManager::class.java)
        val ch = NotificationChannel(CHANNEL, "Grabación", NotificationManager.IMPORTANCE_LOW)
        ch.description = "Aviso mientras ClipLite graba, con los botones Guardar clip y Detener"
        nm.createNotificationChannel(ch)
    }

    private fun servicePI(action: String, req: Int): PendingIntent =
        PendingIntent.getService(this, req, Intent(this, RecorderService::class.java).setAction(action), PendingIntent.FLAG_IMMUTABLE)

    private fun notification(text: String): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val icon = Icon.createWithResource(this, R.drawable.ic_rec)
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_rec)
            .setContentTitle("ClipLite")
            .setContentText(text)
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(icon, "Guardar clip", servicePI(ACTION_CLIP, 1)).build())
            .addAction(Notification.Action.Builder(icon, "Detener", servicePI(ACTION_STOP, 2)).build())
            .build()
    }

    private fun updateNotification(text: String) {
        getSystemService(NotificationManager::class.java).notify(NOTIF_ID, notification(text))
    }

    // ------------------------------------------------------------------ botón flotante

    @SuppressLint("ClickableViewAccessibility")
    private fun showBubble() {
        val wm = getSystemService(WindowManager::class.java)
        val density = resources.displayMetrics.density
        val size = (58 * density).toInt()
        val v = TextView(this).apply {
            text = "●"
            gravity = Gravity.CENTER
            setTextColor(Color.WHITE)
            textSize = 22f
            alpha = 0.8f
            background = GradientDrawable().apply {
                shape = GradientDrawable.OVAL
                setColor(0xE6E5393F.toInt())
                setStroke((2 * density).toInt(), 0x88FFFFFF.toInt())
            }
            contentDescription = "Guardar clip"
        }
        val lp = WindowManager.LayoutParams(
            size, size,
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = (12 * density).toInt()
            y = (140 * density).toInt()
        }
        var downX = 0f
        var downY = 0f
        var startX = 0
        var startY = 0
        var moved = false
        var longFired = false
        val longPress = Runnable {
            longFired = true
            toast("Deteniendo la grabación…")
            stopRecording()
        }
        v.setOnTouchListener { _, ev ->
            when (ev.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    downX = ev.rawX
                    downY = ev.rawY
                    startX = lp.x
                    startY = lp.y
                    moved = false
                    longFired = false
                    main.postDelayed(longPress, 900)
                }
                MotionEvent.ACTION_MOVE -> {
                    val dx = ev.rawX - downX
                    val dy = ev.rawY - downY
                    if (moved || dx * dx + dy * dy > 15 * density * 15 * density) {
                        moved = true
                        main.removeCallbacks(longPress)
                        lp.x = startX + dx.toInt()
                        lp.y = startY + dy.toInt()
                        try { wm.updateViewLayout(v, lp) } catch (_: Exception) {}
                    }
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    main.removeCallbacks(longPress)
                    if (!moved && !longFired && ev.actionMasked == MotionEvent.ACTION_UP) clip()
                }
            }
            true
        }
        try {
            wm.addView(v, lp)
            bubble = v
        } catch (e: Exception) {
            toast("No se pudo mostrar el botón flotante")
        }
    }

    private fun flashBubble() {
        val v = bubble ?: return
        v.animate().scaleX(1.3f).scaleY(1.3f).alpha(1f).setDuration(120).withEndAction {
            v.animate().scaleX(1f).scaleY(1f).alpha(0.8f).setDuration(200).start()
        }.start()
    }

    private fun hideBubble() {
        val v = bubble ?: return
        bubble = null
        try { getSystemService(WindowManager::class.java).removeView(v) } catch (_: Exception) {}
    }
}
