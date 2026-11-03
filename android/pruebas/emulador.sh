#!/usr/bin/env bash
# Prueba de la app en un emulador Android: instala, graba, guarda un clip y comprueba el vídeo.
# La ejecuta GitHub Actions. Deja capturas y resultados en ./resultado.
set -u
APK="$1"
OUT=resultado
mkdir -p "$OUT"
PKG=com.hydrarsw.cliplite
log() { echo "== $*"; }

# Pulsa el elemento cuyo texto o descripción coincide (con la herramienta uiautomator de Android).
tap() {
  for i in 1 2 3 4 5 6 7 8; do
    adb shell uiautomator dump /sdcard/ui.xml >/dev/null 2>&1
    adb pull /sdcard/ui.xml "$OUT/ui.xml" >/dev/null 2>&1
    xy=$(python3 - "$OUT/ui.xml" "$1" <<'PY'
import re, sys
xml = open(sys.argv[1], encoding="utf-8", errors="ignore").read()
pat = re.compile(sys.argv[2], re.I)
for node in re.finditer(r'<node [^>]*>', xml):
    n = node.group(0)
    t = re.search(r' text="([^"]*)"', n); d = re.search(r'content-desc="([^"]*)"', n); r = re.search(r'resource-id="([^"]*)"', n)
    txt = (t.group(1) if t else "") + " " + (d.group(1) if d else "") + " id:" + (r.group(1) if r else "")
    if pat.search(txt):
        b = re.search(r'bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', n)
        if b:
            x1, y1, x2, y2 = map(int, b.groups()); print((x1 + x2) // 2, (y1 + y2) // 2); break
PY
)
    if [ -n "$xy" ]; then adb shell input tap $xy; log "pulsado «$1» en $xy"; return 0; fi
    sleep 1.5
  done
  log "NO encontrado «$1»"; return 1
}
# Botón "Start now" del permiso de captura (botón positivo del diálogo del sistema).
start_now() {
  tap "id:android:id/button1" || tap "Start now|Empezar ahora|Iniciar ahora|Comenzar ahora" || {
    log "uso el teclado para aceptar"; adb shell input keyevent 61; adb shell input keyevent 61; adb shell input keyevent 66; }
  cp "$OUT/ui.xml" "$OUT/ui_permiso.xml" 2>/dev/null
}
shot() { adb exec-out screencap -p > "$OUT/$1.png"; log "captura $1"; }

adb logcat -c
log "instalar"; adb install -r "$APK" || exit 1
adb shell dumpsys package $PKG | grep -E "versionName|versionCode" | head -2
adb shell pm grant $PKG android.permission.RECORD_AUDIO
adb shell pm grant $PKG android.permission.POST_NOTIFICATIONS 2>/dev/null
adb shell appops set $PKG SYSTEM_ALERT_WINDOW allow
# Que el emulador reproduzca sonido mientras graba (tono), para comprobar el audio interno.
log "abrir app"; adb shell am start -W -n $PKG/.MainActivity; sleep 4; shot 1_inicio
tap "Empezar a grabar"; sleep 3; shot 2_permiso
start_now
sleep 8; shot 3_grabando
adb shell dumpsys notification --noredact | grep -A2 "ClipLite" | grep -E "text=|android.text" | head -3
log "guardar clip (botón de la app)"; tap "Guardar clip ahora"; sleep 9; shot 4_clip
log "detener"; tap "Detener grabación"; sleep 5; shot 5_detenido
log "vídeos en la galería:"
adb shell content query --uri content://media/external/video/media --projection _display_name:_size:duration:relative_path | tee "$OUT/galeria.txt"
for f in $(adb shell ls /sdcard/Movies/ClipLite/ 2>/dev/null | tr -d '\r'); do
  adb pull "/sdcard/Movies/ClipLite/$f" "$OUT/$f" >/dev/null && log "descargado $f"
done
# Modo partida completa: cambiar el ajuste y grabar 12 s
log "--- modo partida completa"
adb shell input swipe 500 1500 500 600 300; sleep 1
tap "Partida completa \+ clips"; adb shell input swipe 500 600 500 1500 300; sleep 1
tap "Empezar a grabar"; sleep 3
start_now
sleep 12; tap "Detener grabación"; sleep 6; shot 6_lista
for f in $(adb shell ls /sdcard/Movies/ClipLite/ 2>/dev/null | tr -d '\r'); do
  [ -f "$OUT/$f" ] || { adb pull "/sdcard/Movies/ClipLite/$f" "$OUT/$f" >/dev/null && log "descargado $f"; }
done
log "registro de la app:"
adb logcat -d -s ClipLite:* AndroidRuntime:E | tee "$OUT/logcat.txt" | tail -40
if grep -q "FATAL EXCEPTION" "$OUT/logcat.txt"; then log "LA APP SE CERRÓ"; fi
exit 0
