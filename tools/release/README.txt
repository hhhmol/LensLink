LensLink — телефон Android как веб-камера для Windows 11
=======================================================

Это тестовая версия. Нужны Windows 11 и телефон на Android 8.0 или новее с USB-кабелем.

1. Телефон
   - Включите режим разработчика (Настройки → О телефоне → семь раз нажать на номер сборки)
     и в нём «Отладка по USB». На Xiaomi/POCO/Redmi там же включите «Установка через USB»,
     если она есть.
   - Установите LensLink-<версия>.apk (скопируйте на телефон и откройте; Android попросит
     разрешить установку из этого источника).
   - Откройте приложение LensLink один раз и разрешите доступ к камере.
   - Подключите телефон к компьютеру кабелем и разрешите отладку, когда телефон спросит.

2. Компьютер
   - Распакуйте архив в любую папку (например, C:\LensLink).
   - Чтобы другие программы (Zoom, Teams, OBS, браузер) видели камеру «LensLink», один раз
     запустите install-camera.cmd и согласитесь на права администратора. Без этого шага
     LensLink работает только в своём окне. Убрать камеру: uninstall-camera.cmd.
   - Запустите LensLink.exe. Windows может предупредить, что программа неизвестна
     (у неё пока нет цифровой подписи): «Подробнее» → «Выполнить в любом случае».
   - LensLink сам найдёт телефон и покажет видео. Закрытое окно остаётся в трее.

Как пользоваться
   - Окно перетаскивается за видео, размер меняется за углы, двойной щелчок разворачивает.
   - Настройки спрятаны за краями окна: потяните край, чтобы открыть шторку. Слева камера,
     справа сведения о телефоне и потоке, сверху видео и вид, снизу поведение самих шторок.
   - Любой пункт можно вытащить на видео за его название и оставить там; брошенный обратно
     на открытую шторку, он исчезает.
   - Экспозиция, ISO и баланс белого в крайнем левом положении ползунка — «авто».

Если что-то не работает, пришлите файл %APPDATA%\LensLink\lenslink.log.
В папке лежит adb из Android SDK Platform-Tools (лицензии — в NOTICE-platform-tools.txt).


LensLink — an Android phone as a webcam for Windows 11
======================================================

A test build. Needs Windows 11 and a phone with Android 8.0 or newer, and a USB cable.

1. Phone: enable Developer options and USB debugging (on Xiaomi/POCO/Redmi also "Install via
   USB" if present), install LensLink-<version>.apk, open LensLink once and allow the camera,
   connect the phone and allow USB debugging.
2. PC: unzip anywhere. To let other apps (Zoom, Teams, OBS, browsers) use the "LensLink" camera,
   run install-camera.cmd once (administrator rights); uninstall-camera.cmd removes it. Run
   LensLink.exe (unsigned for now: SmartScreen → "More info" → "Run anyway"). It finds the
   phone by itself; closing the window keeps it in the tray.

Pull a window edge to open its settings curtain; grab an item by its name to keep it on the video.
Problems: send %APPDATA%\LensLink\lenslink.log. adb is from Android SDK Platform-Tools (licenses in
NOTICE-platform-tools.txt).
