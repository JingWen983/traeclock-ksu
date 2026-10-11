@echo off
where gradle >nul 2>nul && (gradle %*) || (echo gradle not found & exit /b 1)
