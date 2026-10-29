@echo off
rem Builds dist\ImageVault.exe (release, x64) and copies the OCR language data.
setlocal
cd /d "%~dp0"
where cargo >nul 2>nul || set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
cargo build --release || exit /b 1
if not exist dist mkdir dist
copy /y target\release\ImageVault.exe dist\ImageVault.exe >nul
rem Tesseract language data for Bangla, Hindi, etc. (shared with the Mac build).
if exist ..\mac\Vendor\tesseract\tessdata (
  if not exist dist\tesseract\tessdata mkdir dist\tesseract\tessdata
  xcopy /y /q ..\mac\Vendor\tesseract\tessdata\*.traineddata dist\tesseract\tessdata\ >nul
)
echo Built dist\ImageVault.exe
