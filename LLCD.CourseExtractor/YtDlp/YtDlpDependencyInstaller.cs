using System;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpDependencyInstaller
    {
        public const string YtDlpWindowsExeUrl = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
        public const string FfmpegReleaseEssentialsZipUrl = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";

        private static readonly HttpClient HttpClient = CreateHttpClient();

        public async Task<YtDlpDependencyInstallResult> InstallAllAsync(string appRootDirectory, CancellationToken cancellationToken = default(CancellationToken), Action<YtDlpDependencyInstallProgress> progress = null)
        {
            if (String.IsNullOrWhiteSpace(appRootDirectory))
                throw new ArgumentException("App root directory is required.", nameof(appRootDirectory));

            var installRoot = Path.Combine(appRootDirectory, "tools");
            Directory.CreateDirectory(installRoot);

            var ytDlpPath = await InstallYtDlpAsync(installRoot, cancellationToken, progress).ConfigureAwait(false);
            var ffmpegPaths = await InstallFfmpegAsync(installRoot, cancellationToken, progress).ConfigureAwait(false);

            progress?.Invoke(new YtDlpDependencyInstallProgress { Message = "Dependency install complete." });
            return new YtDlpDependencyInstallResult
            {
                InstallRoot = installRoot,
                YtDlpPath = ytDlpPath,
                FfmpegPath = ffmpegPaths.FfmpegPath,
                FfprobePath = ffmpegPaths.FfprobePath
            };
        }

        public async Task<string> InstallYtDlpAsync(string toolsRootDirectory, CancellationToken cancellationToken = default(CancellationToken), Action<YtDlpDependencyInstallProgress> progress = null)
        {
            if (String.IsNullOrWhiteSpace(toolsRootDirectory))
                throw new ArgumentException("Tools root directory is required.", nameof(toolsRootDirectory));

            var targetDirectory = Path.Combine(toolsRootDirectory, "yt-dlp");
            Directory.CreateDirectory(targetDirectory);
            var targetPath = Path.Combine(targetDirectory, "yt-dlp.exe");
            var tempPath = targetPath + ".download";

            try
            {
                progress?.Invoke(new YtDlpDependencyInstallProgress { Message = "Downloading yt-dlp..." });
                await DownloadFileAsync(YtDlpWindowsExeUrl, tempPath, "yt-dlp.exe", cancellationToken, progress).ConfigureAwait(false);
                ReplaceFile(tempPath, targetPath);
                progress?.Invoke(new YtDlpDependencyInstallProgress { Message = "yt-dlp installed.", CurrentFile = targetPath });
                return targetPath;
            }
            finally
            {
                TryDelete(tempPath);
            }
        }

        public async Task<(string FfmpegPath, string FfprobePath)> InstallFfmpegAsync(string toolsRootDirectory, CancellationToken cancellationToken = default(CancellationToken), Action<YtDlpDependencyInstallProgress> progress = null)
        {
            if (String.IsNullOrWhiteSpace(toolsRootDirectory))
                throw new ArgumentException("Tools root directory is required.", nameof(toolsRootDirectory));

            var ffmpegDirectory = Path.Combine(toolsRootDirectory, "ffmpeg");
            var binDirectory = Path.Combine(ffmpegDirectory, "bin");
            Directory.CreateDirectory(binDirectory);

            var tempZipPath = Path.Combine(Path.GetTempPath(), "llcd-ffmpeg-" + Guid.NewGuid().ToString("N") + ".zip");
            try
            {
                progress?.Invoke(new YtDlpDependencyInstallProgress { Message = "Downloading FFmpeg essentials..." });
                await DownloadFileAsync(FfmpegReleaseEssentialsZipUrl, tempZipPath, "ffmpeg-release-essentials.zip", cancellationToken, progress).ConfigureAwait(false);

                progress?.Invoke(new YtDlpDependencyInstallProgress { Message = "Extracting FFmpeg..." });
                var ffmpegPath = ExtractExecutableFromZip(tempZipPath, "ffmpeg.exe", binDirectory);
                var ffprobePath = ExtractExecutableFromZip(tempZipPath, "ffprobe.exe", binDirectory);
                progress?.Invoke(new YtDlpDependencyInstallProgress { Message = "FFmpeg installed.", CurrentFile = ffmpegPath });
                return (ffmpegPath, ffprobePath);
            }
            finally
            {
                TryDelete(tempZipPath);
            }
        }

        private static async Task DownloadFileAsync(string url, string targetPath, string currentFile, CancellationToken cancellationToken, Action<YtDlpDependencyInstallProgress> progress)
        {
            using (var response = await HttpClient.GetAsync(url, HttpCompletionOption.ResponseHeadersRead, cancellationToken).ConfigureAwait(false))
            {
                response.EnsureSuccessStatusCode();
                var totalBytes = response.Content.Headers.ContentLength;
                using (var source = await response.Content.ReadAsStreamAsync().ConfigureAwait(false))
                using (var target = new FileStream(targetPath, FileMode.Create, FileAccess.Write, FileShare.None, 81920, true))
                {
                    var buffer = new byte[81920];
                    long received = 0;
                    while (true)
                    {
                        var read = await source.ReadAsync(buffer, 0, buffer.Length, cancellationToken).ConfigureAwait(false);
                        if (read == 0)
                            break;

                        await target.WriteAsync(buffer, 0, read, cancellationToken).ConfigureAwait(false);
                        received += read;
                        progress?.Invoke(new YtDlpDependencyInstallProgress
                        {
                            Message = "Downloading " + currentFile,
                            CurrentFile = currentFile,
                            BytesReceived = received,
                            TotalBytes = totalBytes,
                            Percent = totalBytes.HasValue && totalBytes.Value > 0
                                ? (double?)((double)received / totalBytes.Value * 100)
                                : null
                        });
                    }
                }
            }
        }

        internal static string ExtractExecutableFromZip(string zipPath, string executableName, string targetDirectory)
        {
            using (var archive = ZipFile.OpenRead(zipPath))
            {
                var entry = archive.Entries
                    .Where(item => item.FullName.Replace('\\', '/').EndsWith("/bin/" + executableName, StringComparison.OrdinalIgnoreCase))
                    .OrderBy(item => item.FullName.Length)
                    .FirstOrDefault();

                if (entry == null)
                    throw new FileNotFoundException("Could not find " + executableName + " in FFmpeg archive.");

                var targetPath = Path.Combine(targetDirectory, executableName);
                var tempPath = targetPath + ".download";
                try
                {
                    entry.ExtractToFile(tempPath, true);
                    ReplaceFile(tempPath, targetPath);
                    return targetPath;
                }
                finally
                {
                    TryDelete(tempPath);
                }
            }
        }

        private static void ReplaceFile(string tempPath, string targetPath)
        {
            if (File.Exists(targetPath))
                File.Delete(targetPath);

            File.Move(tempPath, targetPath);
        }

        private static void TryDelete(string path)
        {
            try
            {
                if (File.Exists(path))
                    File.Delete(path);
            }
            catch
            {
            }
        }

        private static HttpClient CreateHttpClient()
        {
            var client = new HttpClient();
            client.DefaultRequestHeaders.UserAgent.ParseAdd("LLCD.DownloaderGUI/1.0");
            return client;
        }
    }
}
