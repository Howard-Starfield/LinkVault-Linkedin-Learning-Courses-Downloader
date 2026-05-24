# LinkVault

> Archive LinkedIn Learning courses and supported generic video links from one desktop app.

## Features

* Modern LinkVault desktop GUI for Windows, macOS, and Linux through Avalonia
* Download in the video quality you like, including 1080p best available, 720p, 540p, or 360p
* Download Exercise files and subtitles automatically
* Download multiple courses at a time
* Automatically import LinkedIn Learning login token from Chrome, Firefox or Microsoft Edge
* Automatically detect the *enterpriseProfileHash* or the  *x-li-identity header* so all organization and library accounts should work
* Generic Video page powered by `yt-dlp` for supported public video URLs and playlists
* Optional generic video/audio sidecars: subtitles, auto captions, thumbnails, and info JSON
* User-triggered `yt-dlp` and FFmpeg tool download into the app-local `tools` folder
* Placeholder navigation for a future LinkedIn Scraper module

## Easy install

Build or run the LinkVault project from the solution.

## Requirements

**Desktop:** .NET 10 runtime or SDK for the GUI build.

**Generic Video page:** `yt-dlp` and FFmpeg are required for generic downloads. The app can detect tools on `PATH`, or you can press `Install Tools` in the Tools page to download:

- `tools/yt-dlp/yt-dlp.exe`
- `tools/ffmpeg/bin/ffmpeg.exe`
- `tools/ffmpeg/bin/ffprobe.exe`

The tool download is never automatic; it only runs after you click `Install Tools`.

## How to use

- **Desktop**

Run `LinkVault.exe`.

The app has these main pages:

1. `LinkedIn Learning`
   - Use this for LinkedIn Learning courses.
   - Log into LinkedIn Learning in Chrome, Firefox, or Edge.
   - Click `Import Token`, choose the browser, enter one course URL per line, choose a download folder, then click `Fetch And Download`.

2. `Generic video`
   - Use this for public or authenticated URLs supported by `yt-dlp`.
   - Paste one or more URLs, one per line.
   - Click `Fetch Metadata` to inspect a single URL.
   - Click `Download` to process the URLs.

3. `Tools`
   - Check or install app-local `yt-dlp` and FFmpeg.

Generic downloads can use browser cookies when you explicitly choose Chrome, Firefox, or Edge in the cookies dropdown. Do this only for sites and content you are allowed to access and download.

## Build and run from source

Install the .NET 10 SDK, then run:

```powershell
dotnet restore LinkVault.sln
dotnet build LinkVault.sln --no-restore
dotnet run --project LLCD.LinkVault\LLCD.LinkVault.csproj
```

Useful verification commands:

```powershell
dotnet test LLCD.CourseExtractor.Tests\LLCD.CourseExtractor.Tests.csproj --no-build
dotnet test LLCD.CourseExtractor.Tests\LLCD.CourseExtractor.Tests.csproj --filter "FullyQualifiedName~YtDlp"
dotnet list LinkVault.sln package --vulnerable --include-transitive
dotnet list LinkVault.sln package --outdated
```

The normal test run skips live LinkedIn/browser-state tests. To run those explicitly, set `LLCD_RUN_LIVE_LINKEDIN_TESTS=1` and provide the relevant secrets through environment variables: `LLCD_TEST_LINKEDIN_TOKEN` or `LLCD_TEST_FIREFOX_TOKEN`, `LLCD_TEST_CHROME_TOKEN`, and `LLCD_TEST_ENTERPRISE_PROFILE_HASH` when the specific assertion needs them.

## Getting the LinkedIn Learning login token cookie

#### You can now extract the token from your browser's default profile if you are logged into LinkedIn by pressing `Extract Token`. If it didn't work for you, manually get the token as follows (Make sure you are logged into LinkedIn Learning first):

* **Firefox**

1. Press `Shift+F9` on your keyboard **OR** right click anywhere on the LinkedIn Learning website , choose "Inspect Element" and click storage.
2. Look for the word "li_at" in the column "Name". Copy the value and paste it in the program.

* **Google Chrome**

1. Right click anywhere on the page and click inspect element **OR** press `F12` on your keyboard
2. Click on the 2 arrows in the top right corner beside the word performance then click Application
3. Double click on the word "Cookies" then click on https://www.linkedin.com
4. Look for the word "li_at" in the column "Name". Copy the value and paste it in the program.

## How to build and run this code on your pc

You don't need to do that if you just want to run the app, but if you want to build your own version:

1. Open visual studio and click on file then Clone Repository.
2. For repository location, use your LinkVault repository URL.
3. Click Clone.
4. The code should be on your pc now. To edit the code, open the `LinkVault.sln` file.
5. Set `LLCD.LinkVault` as the startup project and run it.

## Notes on generic downloads

The Generic Video tab delegates platform support to `yt-dlp`. It does not bypass DRM, paid access, or site restrictions. Subtitles and transcripts are only available when the platform exposes subtitles or automatic captions to `yt-dlp`.

For LinkedIn Learning courses, prefer the native `LinkedIn Learning` tab because it preserves course structure, chapters, exercise files, and LinkedIn transcript output.

## Contributions

I accept any contribution to the codebase whether it is a small bugfix or an exciting new feature as long as it works and fits the scope of the app. Just create a pull request and I will look into it as soon as I can.

## Any Questions? Issues? Recommendations?

Open an issue in the LinkVault repository and I will reply as soon as I can.

## Acknowledgments

- Progress bar from [ShellProgressBar Project](https://github.com/Mpdreamz/shellprogressbar)
- Generic video extraction support through [yt-dlp](https://github.com/yt-dlp/yt-dlp)
- Media merging and conversion through [FFmpeg](https://ffmpeg.org/)
