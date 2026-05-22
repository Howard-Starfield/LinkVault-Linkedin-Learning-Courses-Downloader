using System;
using System.Threading;
using System.Threading.Tasks;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpService
    {
        private readonly YtDlpCommandBuilder _commandBuilder;
        private readonly IYtDlpProcessRunner _processRunner;

        public YtDlpService(string ytDlpPath = "yt-dlp", IYtDlpProcessRunner processRunner = null)
        {
            _commandBuilder = new YtDlpCommandBuilder(ytDlpPath);
            _processRunner = processRunner ?? new YtDlpProcessRunner();
        }

        public async Task<string> GetVersion(CancellationToken cancellationToken = default(CancellationToken))
        {
            var result = await _processRunner.RunAsync(_commandBuilder.BuildVersionCommand(), cancellationToken).ConfigureAwait(false);
            cancellationToken.ThrowIfCancellationRequested();
            if (!result.IsSuccess)
                throw new InvalidOperationException("yt-dlp version check failed: " + result.StandardError);

            return (result.StandardOutput ?? String.Empty).Trim();
        }

        public async Task<YtDlpInfo> GetInfo(string url, YtDlpBrowserCookiesSource cookiesSource = YtDlpBrowserCookiesSource.None, CancellationToken cancellationToken = default(CancellationToken))
        {
            var result = await _processRunner.RunAsync(_commandBuilder.BuildMetadataCommand(url, cookiesSource), cancellationToken).ConfigureAwait(false);
            cancellationToken.ThrowIfCancellationRequested();
            if (!result.IsSuccess)
                throw new InvalidOperationException("yt-dlp metadata fetch failed: " + result.StandardError);

            return YtDlpInfo.FromJson(result.StandardOutput);
        }

        public async Task<YtDlpPlaylistInfo> GetPlaylistInfo(string url, YtDlpBrowserCookiesSource cookiesSource = YtDlpBrowserCookiesSource.None, CancellationToken cancellationToken = default(CancellationToken))
        {
            var result = await _processRunner.RunAsync(_commandBuilder.BuildMetadataCommand(url, cookiesSource, noPlaylist: false), cancellationToken).ConfigureAwait(false);
            cancellationToken.ThrowIfCancellationRequested();
            if (!result.IsSuccess)
                throw new InvalidOperationException("yt-dlp playlist metadata fetch failed: " + result.StandardError);

            return YtDlpPlaylistInfo.FromJson(result.StandardOutput);
        }

        public Task<YtDlpProcessResult> Download(YtDlpDownloadOptions options, CancellationToken cancellationToken = default(CancellationToken))
        {
            return Download(options, null, null, cancellationToken);
        }

        public Task<YtDlpProcessResult> Download(YtDlpDownloadOptions options, Action<string> outputLine, Action<string> errorLine, CancellationToken cancellationToken = default(CancellationToken))
        {
            return _processRunner.RunAsync(_commandBuilder.BuildDownloadCommand(options), cancellationToken, outputLine, errorLine);
        }
    }
}
