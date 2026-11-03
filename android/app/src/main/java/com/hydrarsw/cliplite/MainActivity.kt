package com.hydrarsw.cliplite

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.media.projection.MediaProjectionManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.text.format.Formatter
import android.util.Size
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.ScrollView
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast

/** Pantalla principal: empezar/parar, ajustes, clips guardados y actualizaciones. */
class MainActivity : Activity() {
    private val bg = Color.parseColor("#0F1014")
    private val panel = Color.parseColor("#16171D")
    private val panel2 = Color.parseColor("#1D1F27")
    private val text = Color.parseColor("#E8E9EE")
    private val dim = Color.parseColor("#9497A3")
    private val accent = Color.parseColor("#E5393F")

    private lateinit var status: TextView
    private lateinit var startBtn: Button
    private lateinit var clipBtn: Button
    private lateinit var clipList: LinearLayout
    private lateinit var updateBox: LinearLayout

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    private fun round(color: Int, radius: Int = 12) = GradientDrawable().apply {
        setColor(color)
        cornerRadius = dp(radius).toFloat()
    }

    private fun label(s: String, size: Float = 15f, color: Int = text, bold: Boolean = false) = TextView(this).apply {
        this.text = s
        textSize = size
        setTextColor(color)
        if (bold) typeface = Typeface.DEFAULT_BOLD
    }

    private fun button(s: String, color: Int = panel2, onClick: () -> Unit) = Button(this).apply {
        this.text = s
        isAllCaps = false
        setTextColor(Color.WHITE)
        background = round(color, 10)
        setPadding(dp(14), dp(10), dp(14), dp(10))
        setOnClickListener { onClick() }
    }

    private fun card(title: String?): LinearLayout = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        background = round(panel)
        setPadding(dp(16), dp(14), dp(16), dp(14))
        layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = dp(12) }
        if (title != null) addView(label(title, 17f, bold = true))
    }

    private fun choice(parent: LinearLayout, title: String, options: List<Pair<String, Any>>, current: Any, key: String) {
        parent.addView(label(title, 13f, dim).apply { setPadding(0, dp(12), 0, dp(4)) })
        val g = RadioGroup(this).apply { orientation = if (options.size > 2) RadioGroup.HORIZONTAL else RadioGroup.VERTICAL }
        for ((name, value) in options) {
            val rb = RadioButton(this).apply {
                this.text = name
                setTextColor(text)
                id = View.generateViewId()
                isChecked = value == current
                setOnCheckedChangeListener { _, on -> if (on) Prefs.set(this@MainActivity, key, value) }
            }
            g.addView(rb)
        }
        parent.addView(g)
    }

    @Suppress("DEPRECATION")
    private fun toggle(parent: LinearLayout, title: String, on: Boolean, onChange: (Boolean) -> Unit): Switch {
        val s = Switch(this).apply {
            this.text = title
            setTextColor(text)
            textSize = 15f
            isChecked = on
            setPadding(0, dp(8), 0, dp(8))
            setOnCheckedChangeListener { _, v -> onChange(v) }
        }
        parent.addView(s)
        return s
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.statusBarColor = bg
        window.navigationBarColor = bg
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(20), dp(16), dp(28))
        }
        val scroll = ScrollView(this).apply {
            setBackgroundColor(bg)
            addView(root)
        }
        setContentView(scroll)

        // Cabecera
        val head = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        head.addView(label("● ", 22f, accent, true))
        head.addView(label("ClipLite", 24f, bold = true))
        head.addView(label("  " + Updater.currentVersion(this), 13f, dim))
        root.addView(head)

        // Aviso de actualización (oculto hasta que haya una)
        updateBox = card(null).apply {
            visibility = View.GONE
            background = round(Color.parseColor("#1F2A22"))
        }
        root.addView(updateBox)

        // Estado y botones principales
        val main = card(null)
        status = label("", 15f, dim)
        main.addView(status)
        startBtn = button("Empezar a grabar", accent) { onStartStop() }
        startBtn.textSize = 18f
        main.addView(startBtn, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(60)).apply { topMargin = dp(12) })
        clipBtn = button("Guardar clip ahora") { startService(Intent(this, RecorderService::class.java).setAction(RecorderService.ACTION_CLIP)) }
        main.addView(clipBtn, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = dp(8) })
        main.addView(label("Mientras juegas: toca el botón rojo flotante para guardar los últimos segundos; mantenlo pulsado para detener. También desde la notificación.", 13f, dim).apply { setPadding(0, dp(10), 0, 0) })
        root.addView(main)

        // Ajustes
        val c = Prefs.load(this)
        val set = card("Ajustes")
        choice(set, "Qué grabar", listOf("Solo clips (los últimos segundos)" to false, "Partida completa + clips" to true), c.fullMatch, "partida")
        choice(set, "Duración del clip", listOf("15 s" to 15, "30 s" to 30, "60 s" to 60), c.clipSeconds, "segundos")
        choice(set, "Calidad", listOf("720p" to 720, "1080p" to 1080), c.shortSide, "calidad")
        choice(set, "Fotogramas por segundo", listOf("30" to 30, "60" to 60), c.fps, "fps")
        set.addView(View(this), LinearLayout.LayoutParams(1, dp(8)))
        toggle(set, "Sonido del juego", c.audio) { Prefs.set(this, "audio", it) }
        toggle(set, "Mi micrófono", c.mic) { Prefs.set(this, "mic", it) }
        toggle(set, "Botón flotante sobre el juego", c.bubble) { on ->
            Prefs.set(this, "burbuja", on)
            if (on && !Settings.canDrawOverlays(this)) {
                Toast.makeText(this, "Activa «Mostrar sobre otras apps» para ClipLite", Toast.LENGTH_LONG).show()
                startActivity(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName")))
            }
        }
        set.addView(label("Los cambios se aplican la próxima vez que empieces a grabar. Algunos juegos no permiten grabar su sonido.", 12f, dim).apply { setPadding(0, dp(6), 0, 0) })
        root.addView(set)

        // Clips
        val clipsCard = card("Mis clips")
        clipsCard.addView(label("Se guardan en la galería, carpeta Movies/ClipLite.", 12f, dim))
        clipList = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        clipsCard.addView(clipList)
        root.addView(clipsCard)

        // Actualizaciones
        val upd = card(null)
        upd.addView(button("Buscar actualizaciones") { checkUpdates(true) })
        root.addView(upd)

        checkUpdates(false)
    }

    override fun onResume() {
        super.onResume()
        RecorderService.onChange = { runOnUiThread { refresh() } }
        refresh()
    }

    override fun onPause() {
        RecorderService.onChange = null
        super.onPause()
    }

    private fun refresh() {
        val rec = RecorderService.isRunning
        status.text = if (rec) "● Grabando. Abre tu juego." else "Listo para grabar."
        status.setTextColor(if (rec) accent else dim)
        startBtn.text = if (rec) "Detener grabación" else "Empezar a grabar"
        startBtn.background = round(if (rec) panel2 else accent, 10)
        clipBtn.visibility = if (rec) View.VISIBLE else View.GONE
        loadClips()
    }

    // ------------------------------------------------------------------ empezar / parar

    private fun onStartStop() {
        if (RecorderService.isRunning) {
            startService(Intent(this, RecorderService::class.java).setAction(RecorderService.ACTION_STOP))
            return
        }
        val need = mutableListOf(Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= 33) need.add(Manifest.permission.POST_NOTIFICATIONS)
        val missing = need.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) {
            requestPermissions(missing.toTypedArray(), 1)
        } else {
            askCapture()
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == 1) {
            if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                Toast.makeText(this, "Sin permiso de audio se grabará sin sonido", Toast.LENGTH_LONG).show()
            }
            askCapture()
        }
    }

    @Suppress("DEPRECATION")
    private fun askCapture() {
        val mpm = getSystemService(MediaProjectionManager::class.java)
        startActivityForResult(mpm.createScreenCaptureIntent(), 2)
    }

    @Deprecated("Activity sin AndroidX")
    @Suppress("DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != 2) return
        if (resultCode != RESULT_OK || data == null) {
            Toast.makeText(this, "Hace falta permitir la grabación de pantalla", Toast.LENGTH_SHORT).show()
            return
        }
        val i = Intent(this, RecorderService::class.java)
            .setAction(RecorderService.ACTION_START)
            .putExtra(RecorderService.EXTRA_CODE, resultCode)
            .putExtra(RecorderService.EXTRA_DATA, data)
        startForegroundService(i)
        Toast.makeText(this, "Grabando. Ya puedes abrir tu juego.", Toast.LENGTH_SHORT).show()
        status.postDelayed({ refresh() }, 800)
    }

    // ------------------------------------------------------------------ clips

    private fun loadClips() {
        Thread {
            val clips = try { ClipStore.list(this) } catch (e: Exception) { emptyList() }
            runOnUiThread { showClips(clips) }
        }.start()
    }

    private fun showClips(clips: List<ClipStore.Clip>) {
        clipList.removeAllViews()
        if (clips.isEmpty()) {
            clipList.addView(label("Todavía no hay clips.", 14f, dim).apply { setPadding(0, dp(10), 0, 0) })
            return
        }
        for (c in clips.take(60)) {
            val row = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                background = round(panel2, 10)
                setPadding(dp(8), dp(8), dp(8), dp(8))
                layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = dp(8) }
            }
            val thumb = ImageView(this).apply {
                scaleType = ImageView.ScaleType.CENTER_CROP
                background = round(Color.BLACK, 6)
                clipToOutline = true
            }
            row.addView(thumb, LinearLayout.LayoutParams(dp(112), dp(63)))
            Thread {
                val bmp = try { contentResolver.loadThumbnail(c.uri, Size(320, 180), null) } catch (e: Exception) { null }
                if (bmp != null) runOnUiThread { thumb.setImageBitmap(bmp) }
            }.start()
            val info = LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(dp(10), 0, 0, 0)
            }
            val nice = c.name.removePrefix("ClipLite_").removeSuffix(".mp4").replace('_', ' ')
            info.addView(label(nice, 13f, bold = true))
            val secs = c.durationMs / 1000
            info.addView(label("%d:%02d · %s".format(secs / 60, secs % 60, Formatter.formatShortFileSize(this, c.size)), 12f, dim))
            val actions = LinearLayout(this).apply { setPadding(0, dp(4), 0, 0) }
            actions.addView(smallAction("Ver") {
                startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(c.uri, "video/mp4").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
            })
            actions.addView(smallAction("Compartir") {
                val send = Intent(Intent.ACTION_SEND).setType("video/mp4").putExtra(Intent.EXTRA_STREAM, c.uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                startActivity(Intent.createChooser(send, "Compartir clip"))
            })
            actions.addView(smallAction("Borrar") {
                ClipStore.delete(this, c.uri)
                loadClips()
            })
            info.addView(actions)
            row.addView(info, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
            clipList.addView(row)
        }
    }

    private fun smallAction(s: String, onClick: () -> Unit) = TextView(this).apply {
        this.text = s
        textSize = 13f
        setTextColor(Color.parseColor("#FF6A5C"))
        setPadding(0, dp(4), dp(18), dp(4))
        setOnClickListener { onClick() }
    }

    // ------------------------------------------------------------------ actualizaciones

    private fun checkUpdates(manual: Boolean) {
        Thread {
            val r = Updater.latest()
            val current = Updater.currentVersion(this)
            runOnUiThread {
                when {
                    r != null && Updater.isNewer(r.version, current) -> showUpdate(r)
                    manual && r != null -> Toast.makeText(this, "ClipLite está al día ($current)", Toast.LENGTH_SHORT).show()
                    manual -> Toast.makeText(this, "No se pudo comprobar. ¿Hay internet?", Toast.LENGTH_SHORT).show()
                }
            }
        }.start()
    }

    private fun showUpdate(r: Updater.Release) {
        updateBox.removeAllViews()
        updateBox.visibility = View.VISIBLE
        updateBox.addView(label("⬆ Hay una actualización nueva", 16f, bold = true))
        updateBox.addView(label("ClipLite ${r.version}" + (r.notes.lineSequence().firstOrNull()?.let { if (it.isNotBlank()) " — $it" else "" } ?: ""), 13f, dim))
        val progress = label("", 13f, dim)
        val btn = button("Actualizar ahora", Color.parseColor("#2F8A52")) {}
        btn.setOnClickListener {
            if (!packageManager.canRequestPackageInstalls()) {
                Toast.makeText(this, "Permite a ClipLite instalar actualizaciones y vuelve a pulsar", Toast.LENGTH_LONG).show()
                startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName")))
                return@setOnClickListener
            }
            btn.isEnabled = false
            progress.text = "Descargando…"
            Thread {
                try {
                    val apk = Updater.download(this, r) { p -> runOnUiThread { progress.text = "Descargando… $p %" } }
                    runOnUiThread { progress.text = "Verificada ✓ Instalando…" }
                    Updater.install(this, apk)
                } catch (e: Exception) {
                    runOnUiThread {
                        progress.text = "No se pudo actualizar: ${e.message}"
                        btn.isEnabled = true
                    }
                }
            }.start()
        }
        updateBox.addView(btn, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = dp(10) })
        updateBox.addView(progress)
    }
}
