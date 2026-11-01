@echo off
chcp 65001 >nul
title Compilar ClipLite
cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
    echo No tienes Rust instalado.
    echo Se abrira https://rustup.rs : descarga rustup-init.exe, ejecutalo y acepta las opciones por defecto
    echo ^(si te ofrece instalar "Visual Studio Build Tools", acepta^). Luego vuelve a abrir este archivo.
    start https://rustup.rs
    pause
    exit /b 1
)

echo Compilando ClipLite en modo optimizado...
cargo build --release
if errorlevel 1 (
    echo.
    echo La compilacion fallo. Revisa los mensajes de arriba.
    pause
    exit /b 1
)

copy /Y "target\release\ClipLite.exe" "ClipLite.exe" >nul
echo.
echo Listo: ClipLite.exe
if not exist "ffmpeg.exe" (
    echo.
    echo FALTA ffmpeg.exe: descarga "ffmpeg-release-essentials.zip" de https://www.gyan.dev/ffmpeg/builds/
    echo y copia bin\ffmpeg.exe en esta misma carpeta.
)
pause
