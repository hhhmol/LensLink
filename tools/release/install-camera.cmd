@echo off
rem Installs (or updates) the LensLink virtual camera. Windows asks for administrator rights.
powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File \"%~dp0install-vcam.ps1\" -Pause'"
