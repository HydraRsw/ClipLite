# ClipLite

Grabador de partidas y clips para Windows, al estilo de Outplayed, diseñado para que **no notes que está abierto**.

- Empieza a grabar solo cuando abres un juego y para cuando lo cierras.
- Dos modos (se cambian desde el icono de la bandeja):
  - **Solo momentos importantes**: guarda clips de tus kills, multikills, objetivos… y nada más.
  - **Partida completa**: guarda la partida entera en un vídeo (y además los clips por separado).
- **Ctrl+F10** guarda un clip de los últimos 30 s en cualquier juego. **Ctrl+F9** empieza/para a mano.
- Eliges la carpeta donde se guarda todo.
- **Al cerrar el juego se abre el reproductor** con la partida, sus clips y una línea de tiempo con cada kill, multikill u objetivo marcados. Haz clic en un marcador para saltar a ese momento. También se abre con doble clic en el icono o desde "Ver mis partidas y clips".

## Por qué casi no consume recursos

| | |
|---|---|
| Captura de pantalla | Desktop Duplication de Windows: la imagen se queda en la GPU |
| Codificación | Chip codificador dedicado de la GPU (NVENC en NVIDIA, AMF en AMD, Quick Sync en Intel). Es un circuito aparte del que dibuja el juego, por eso no baja los FPS |
| Programa | Rust sin librerías externas, menos de 1 MB. Mientras graba: <1 % de un núcleo y ~3 MB de RAM. En espera: mira la lista de procesos cada 3 s y nada más |
| Clips | Se graba en trozos de 2 s; un clip es unir trozos copiando bytes, sin volver a codificar. Tarda milisegundos |
| Prioridad | ClipLite corre en prioridad "por debajo de lo normal" para ceder siempre la CPU al juego |

Es la misma técnica que usan ShadowPlay y OBS con NVENC.

## Instalación

**Opción A — sin instalar nada (recomendada):** sube esta carpeta a un repositorio de GitHub. En la pestaña **Actions** se compila sola y genera `ClipLite-Windows.zip` con `ClipLite.exe` y `ffmpeg.exe` ya incluidos. Descomprímelo y abre `ClipLite.exe`.

**Opción B — compilar en tu PC:**
1. Instala Rust desde https://rustup.rs (acepta las opciones por defecto).
2. Doble clic en `compilar.bat` → genera `ClipLite.exe`.
3. Descarga `ffmpeg-release-essentials.zip` de https://www.gyan.dev/ffmpeg/builds/ (FFmpeg 6.1 o superior) y copia `bin\ffmpeg.exe` junto a `ClipLite.exe`.
4. Abre `ClipLite.exe`. Aparece un círculo gris junto al reloj (rojo cuando graba).

La primera vez prueba qué codificador funciona en tu GPU (lo verás en el menú: "Codificador: …").

## Uso

Clic (izquierdo o derecho) en el icono de la bandeja:

- Grabar la partida completa / Solo momentos importantes
- Cambiar carpeta de guardado…
- Abrir carpeta de clips
- Empezar/detener grabación, Guardar clip
- Editar configuración / Recargar configuración
- Iniciar con Windows

Los vídeos quedan así:

```
<tu carpeta>\
  League of Legends\
    Clips\      League of Legends_2026-11-01_21-14-03_pentakill.mp4
    Partidas\   League of Legends_2026-11-01_20-45-10.mp4
```

## Momentos automáticos

| Juego | Cómo se detecta | Qué detecta |
|---|---|---|
| League of Legends | API oficial local del juego | kills, double/triple/quadra/penta, ace, primera sangre, dragón, barón, heraldo, larvas, torres, inhibidores; opcional muertes y asistencias |
| Counter-Strike 2 | Game State Integration oficial de Valve (ClipLite instala el archivo `gamestate_integration_cliplite.cfg` solo) | kills, 2k/3k/4k/ace; opcional muertes |
| Cualquier otro juego | Atajo **Ctrl+F10** | lo que tú decidas |

Ambas son APIs oficiales y de solo lectura: ClipLite no lee la memoria del juego ni inyecta nada, así que no tiene riesgo con anti-cheats (Vanguard, VAC, EAC…). VALORANT, Fortnite, Apex, etc. no ofrecen una API local de eventos, por eso en ellos se usa el atajo.

Kills seguidas se unen en un único clip con la etiqueta del mejor momento (por ejemplo, una kill seguida de una doble kill se guarda como un solo clip "doublekill").

## Configuración

`%APPDATA%\ClipLite\config.ini` (menú → Editar configuración). Todo está comentado. Lo más útil:

| Clave | Por defecto | |
|---|---|---|
| `carpeta` | `Vídeos\ClipLite` | dónde se guarda |
| `modo` | `momentos` | `momentos` o `completa` |
| `fps` / `bitrate_mbps` | 60 / 20 | 1080p: 15–25 · 1440p: 25–35 · 4K: 40–60 |
| `codec` | `h264` | `hevc` ocupa la mitad (algunas apps no lo reproducen) |
| `segundos_antes` / `segundos_despues` | 15 / 5 | duración de los clips automáticos |
| `segundos_clip_manual` | 30 | lo que guarda Ctrl+F10 |
| `abrir_reproductor` | si | abrir el reproductor al cerrar el juego |
| `pistas_por_aplicacion` | si | guardar juego y aplicaciones en pistas separadas |
| `apps_con_pista_propia` | Discord.exe, Spotify.exe | añade el `.exe` de cualquier app (p. ej. `chrome.exe`) |
| `microfono` | no | grabar tu micrófono en su propia pista |
| `borrar_videos_de_mas_de_dias` | 0 | borrar solos los vídeos de partidas con más días (0 = nunca) |
| `limite_espacio_gb` | 0 | tope de espacio; se borra lo más antiguo (0 = sin límite) |
| `atajo_clip` / `atajo_grabar` | Ctrl+F10 / Ctrl+F9 | |
| `juegos` | lista | añade el `.exe` de cualquier juego para que lo detecte |
| `eventos_lol` | kills, multikills, objetivos | añade `muertes`, `asistencias` si quieres |
| `desfase_audio_ms` | 0 | ajuste fino si notas el audio adelantado o atrasado |

Espacio en disco: en modo momentos solo hay ~1 minuto de búfer temporal. En modo completa, a 20 Mbps una hora ocupa ~9 GB.

## Pistas de audio por aplicación

Cada fuente de sonido se guarda en su propia pista dentro del vídeo:

| Pista | Qué contiene |
|---|---|
| 1 · Todo | la mezcla completa (lo que oíste). Es la que reproducen Discord, WhatsApp, etc. |
| Juego | solo el juego |
| Discord, Spotify… | cada aplicación de `apps_con_pista_propia` que esté abierta al empezar la partida |
| Micrófono | tu voz, si activas `microfono = si` (también se mezcla en la pista 1) |

En el reproductor aparecen como casillas debajo del vídeo: desmarca **Discord** y lo oirás sin las voces al instante. Con **Guardar copia con este audio** se crea un vídeo nuevo (`…_sin-Discord.mp4`) junto al original, listo para compartir. El original no se toca.

- Al quitar pistas se oyen solo las pistas marcadas: los sonidos de aplicaciones sin pista propia (por ejemplo, un vídeo de YouTube) solo están en "Todo".
- Editores como DaVinci Resolve o Premiere muestran cada pista con su nombre.
- Necesita Windows 11 o Windows 10 versión 2004 o posterior. En versiones anteriores se graba solo la pista "Todo", como siempre.

## Borrar vídeos viejos

- **Automático:** clic derecho en el icono › **Borrar vídeos viejos automáticamente** › Nunca / 7 / 14 / 30 / 90 días. Se revisa al abrir ClipLite y después de cada partida. También puedes poner un tope de espacio con `limite_espacio_gb` en la configuración: si los vídeos lo superan, se borran empezando por los más antiguos.
- **A mano, desde el reproductor:**
  - 🗑 en cada clip, o **Borrar partida** arriba.
  - En **Espacio y limpieza** (barra lateral) ves cuánto ocupa todo y puedes borrar de golpe las partidas de más de X días.
- **Seguridad:**
  - Los clips marcados como **favoritos (☆) nunca se borran** automáticamente ni al borrar una partida entera.
  - Lo que borras a mano va a la **Papelera de reciclaje**; la limpieza automática borra definitivamente, porque si no, no liberaría espacio.
  - Solo se tocan vídeos creados por ClipLite.

## Reproductor

Se abre como ventana de aplicación con Microsoft Edge, que ya viene con Windows, y lo sirve ClipLite desde tu propio PC (`127.0.0.1`, inaccesible desde fuera y protegido con una clave aleatoria). Cuando está cerrado no consume nada. También existe `ClipLite.html` en tu carpeta de vídeos, que funciona sin ClipLite abierto pero sin poder silenciar pistas.

- Arriba, las partidas (número, juego, fecha, kills, clips y duración). Filtra por juego o busca.
- En el centro, la partida completa o el clip elegido.
- Abajo, la línea de tiempo con los momentos marcados; las franjas rojas son los clips guardados.
- A la derecha, los clips de la partida. ☆ los marca como favoritos.
- Teclas: `Espacio` pausa · `←` `→` 5 s · `,` `.` fotograma a fotograma · `N` `P` siguiente/anterior momento · `F` pantalla completa · `M` silencio.

Si no quieres que se abra solo, pon `abrir_reproductor = no` en la configuración.

## Problemas frecuentes

- **Pantalla negra en un juego**: ponlo en "Ventana sin bordes" / "Pantalla completa sin bordes". La pantalla completa exclusiva antigua no siempre se puede capturar.
- **"AcquireNextFrame failed: 887a0026" en el registro**: Windows le quitó la pantalla a la captura (cambio a pantalla completa, alt-tab, aviso de UAC). ClipLite la reanuda en menos de un segundo dentro de la misma partida. Si pasa a menudo, usa "Sin bordes".
- **El atajo no funciona**: otro programa lo usa (por ejemplo, la superposición de NVIDIA usa Alt+F9 y Alt+F10). Cámbialo en la configuración.
- **CS2 no genera clips**: reinicia CS2 tras abrir ClipLite por primera vez (necesita leer el archivo de integración al arrancar).
- **Cualquier otra cosa**: menú → Ver registro.

## Estructura del código

| Archivo | Qué hace |
|---|---|
| `main.rs` | arranque |
| `win.rs` | todo lo de Windows: bandeja, atajos, WASAPI (audio del sistema, por aplicación y micrófono), procesos, WinHTTP, selector de carpeta |
| `controller.rs` | decide cuándo grabar y qué guardar |
| `recorder.rs` | FFmpeg, segmentos, clips, partida completa, recuperación tras un cierre inesperado |
| `audio.rs` | mezclador multipista: sincroniza todas las fuentes con el vídeo y las envía a FFmpeg |
| `events.rs` | detección de momentos de LoL y CS2 |
| `library.rs`, `player.html` | biblioteca de partidas y reproductor |
| `server.rs` | servidor local del reproductor: vídeos, mezcla de pistas y copias |
| `config.rs`, `json.rs`, `util.rs`, `log.rs` | utilidades |
| `headless.rs` | banco de pruebas que simula una partida (solo fuera de Windows) |
