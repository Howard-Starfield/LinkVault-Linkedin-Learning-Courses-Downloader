using System;
using System.Collections.Generic;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpCommandBuilder
    {
        private readonly string _ytDlpPath;

        public YtDlpCommandBuilder(string ytDlpPath = "yt-dlp")
        {
            _ytDlpPath = String.IsNullOrWhiteSpace(ytDlpPath) ? "yt-dlp" : ytDlpPath;
        }

        public YtDlpCommand BuildVersionCommand()
        {
            return new YtDlpCommand(_ytDlpPath, new[] { "--version" });
        }

        public YtDlpCommand BuildMetadataCommand(string url, YtDlpBrowserCookiesSource cookiesSource = YtDlpBrowserCookiesSource.None, bool noPlaylist = true)
        {
            if (String.IsNullOrWhiteSpace(url))
                throw new ArgumentException("URL is required.", nameof(url));

            var args = new List<string>();
            args.Add("-J");
            if (noPlaylist)
            {
                args.Add("--no-playlist");
            }
            AddCookies(args, cookiesSource);
            args.Add(url);
            return new YtDlpCommand(_ytDlpPath, args);
        }

        public YtDlpCommand BuildDownloadCommand(YtDlpDownloadOptions options)
        {
            if (options is null)
                throw new ArgumentNullException(nameof(options));
            if (String.IsNullOrWhiteSpace(options.Url))
                throw new ArgumentException("URL is required.", nameof(options));
            if (String.IsNullOrWhiteSpace(options.OutputTemplate))
                throw new ArgumentException("Output template is required.", nameof(options));

            var args = new List<string>();
            args.Add("--newline");
            if (options.NoPlaylist)
            {
                args.Add("--no-playlist");
            }
            args.Add("-o");
            args.Add(options.OutputTemplate);

            AddCookies(args, options.CookiesSource);
            AddFfmpegLocation(args, options.FfmpegLocation);
            AddSubtitleOptions(args, options);
            AddSidecarOptions(args, options);

            if (options.FormatChoice == YtDlpFormatChoice.Audio)
            {
                args.Add("-x");
                args.Add("--audio-format");
                args.Add(ToAudioFormatArgument(options.AudioFormat));
            }
            else
            {
                args.Add("-f");
                args.Add(String.IsNullOrWhiteSpace(options.FormatId)
                    ? "bestvideo+bestaudio/best"
                    : options.FormatId + "+bestaudio/best");
                args.Add("--merge-output-format");
                args.Add("mp4");
            }

            args.Add(options.Url);
            return new YtDlpCommand(_ytDlpPath, args);
        }

        private static void AddFfmpegLocation(List<string> args, string ffmpegLocation)
        {
            if (String.IsNullOrWhiteSpace(ffmpegLocation))
                return;

            args.Add("--ffmpeg-location");
            args.Add(ffmpegLocation);
        }

        private static void AddCookies(List<string> args, YtDlpBrowserCookiesSource cookiesSource)
        {
            if (cookiesSource == YtDlpBrowserCookiesSource.None)
                return;

            args.Add("--cookies-from-browser");
            args.Add(ToCookiesArgument(cookiesSource));
        }

        private static void AddSubtitleOptions(List<string> args, YtDlpDownloadOptions options)
        {
            if (options.WriteSubtitles)
            {
                args.Add("--write-subs");
            }
            if (options.WriteAutomaticSubtitles)
            {
                args.Add("--write-auto-subs");
            }
            if (options.WriteSubtitles || options.WriteAutomaticSubtitles)
            {
                if (!String.IsNullOrWhiteSpace(options.SubtitleLanguages))
                {
                    args.Add("--sub-langs");
                    args.Add(options.SubtitleLanguages);
                }
                if (options.ConvertSubtitlesToSrt)
                {
                    args.Add("--convert-subs");
                    args.Add("srt");
                }
            }
        }

        private static void AddSidecarOptions(List<string> args, YtDlpDownloadOptions options)
        {
            if (options.WriteInfoJson)
            {
                args.Add("--write-info-json");
            }
            if (options.WriteThumbnail)
            {
                args.Add("--write-thumbnail");
            }
        }

        private static string ToCookiesArgument(YtDlpBrowserCookiesSource cookiesSource)
        {
            switch (cookiesSource)
            {
                case YtDlpBrowserCookiesSource.Chrome:
                    return "chrome";
                case YtDlpBrowserCookiesSource.Firefox:
                    return "firefox";
                case YtDlpBrowserCookiesSource.Edge:
                    return "edge";
                default:
                    throw new ArgumentOutOfRangeException(nameof(cookiesSource), cookiesSource, null);
            }
        }

        private static string ToAudioFormatArgument(YtDlpAudioFormat audioFormat)
        {
            switch (audioFormat)
            {
                case YtDlpAudioFormat.Mp3:
                    return "mp3";
                case YtDlpAudioFormat.M4a:
                    return "m4a";
                case YtDlpAudioFormat.Flac:
                    return "flac";
                case YtDlpAudioFormat.Wav:
                    return "wav";
                default:
                    throw new ArgumentOutOfRangeException(nameof(audioFormat), audioFormat, null);
            }
        }
    }
}
