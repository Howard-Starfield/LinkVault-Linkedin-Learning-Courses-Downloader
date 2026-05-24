# Issues And Fixes

This is the simple list of problems we found and what we changed.

## LinkedIn Token

Problem: The user had to open browser developer tools and copy the token by hand.

Fix: The app now tries to find a valid LinkedIn token from Chrome, Edge, or Firefox by itself.

Result: The user can click less and does not need to hunt for the token.

## Exercise File Download

Problem: Exercise files failed because LinkedIn changed the download link style.

Fix: The app now checks the real LinkedIn course page for the newer download link.

Result: Exercise files download again.

## Ambry Links

Problem: LinkedIn now uses links like `linkedin.com/ambry/?x-li-ambry-ep=...`, and the app did not understand them.

Fix: The app now finds and uses those Ambry links.

Result: The newer exercise file links work.

## Failed Exercise File Crash Chain

Problem: When one exercise file failed, the downloader could break and later video downloads could fail too.

Fix: Each exercise file is retried by itself. If it still fails, the app writes a failure note and keeps going.

Result: One bad file does not ruin the whole course download.

## Zip Files

Problem: Exercise files downloaded as zip files and stayed zipped.

Fix: The app now unzips exercise files after download.

Result: The user gets a normal folder they can open.

## Duplicate Exercise Folder

Problem: Some zips became folders like `Ex_Files/Ex_Files/...`.

Fix: If the zip already contains a folder with the same name, the app removes the extra wrapper.

Result: The exercise files land in one clean folder.

## Zip Safety

Problem: A bad zip file could try to put files in the wrong place.

Fix: The app checks every file path before extracting.

Result: Zip extraction is safer and does not write outside the course folder.

## Delete Zip After Unzip

Problem: Keeping both the zip and extracted folder made clutter.

Fix: The app deletes the zip only after unzip succeeds.

Result: The folder stays clean, and failed zips are kept so the user does not lose data.

## Video Resolution

Problem: The old label said video quality, but users needed a clear resolution choice.

Fix: The app now shows `Video resolution` and includes best available, 720, 540, and 360 choices.

Result: The user can pick the video size more clearly.

## Skip Video Downloads

Problem: The app always downloaded videos.

Fix: The app now has a `Download videos` checkbox. It is on by default.

Result: The user can download only exercise files if they want.

## YouTube / Generic Video Cookies

Problem: `yt-dlp` could fail when Chrome locked its cookie database.

Fix: The app retries public videos without browser cookies when that known cookie error happens.

Result: Public video downloads are less likely to fail.

## Dependency Tools

Problem: Users might not have helper tools like `yt-dlp` or `ffmpeg`.

Fix: The app has tool checks and a download/install path for dependencies.

Result: Setup is easier.

## Tests And Checks

Problem: We needed proof the fixes did not break the app.

Fix: Added tests for Ambry links, exercise zip extraction, unsafe zip paths, and skipping video details.

Result: The main checks passed after the fixes.
