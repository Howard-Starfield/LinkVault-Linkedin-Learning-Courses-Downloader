using System;
using System.Globalization;
using System.Text.RegularExpressions;

namespace LLCD.CourseExtractor.YtDlp
{
    public static class YtDlpProgressParser
    {
        private static readonly Regex DownloadProgressRegex = new Regex(
            @"^\[download\]\s+(?<percent>\d+(?:\.\d+)?)%\s+of\s+(?<size>.+?)(?:\s+at\s+(?<speed>.+?))?(?:\s+ETA\s+(?<eta>\S+))?(?:\s+in\s+(?<elapsed>\S+).*)?$",
            RegexOptions.Compiled | RegexOptions.CultureInvariant);

        private static readonly Regex DestinationRegex = new Regex(
            @"^\[(?:download|ExtractAudio)\]\s+Destination:\s+(?<path>.+)$",
            RegexOptions.Compiled | RegexOptions.CultureInvariant);

        private static readonly Regex MergerRegex = new Regex(
            @"^\[Merger\]\s+Merging formats into\s+""(?<path>.+)""",
            RegexOptions.Compiled | RegexOptions.CultureInvariant);

        public static YtDlpProgress ParseLine(string line)
        {
            if (String.IsNullOrWhiteSpace(line))
                return null;

            var progressMatch = DownloadProgressRegex.Match(line);
            if (progressMatch.Success)
            {
                return new YtDlpProgress
                {
                    Status = YtDlpJobStatus.Downloading,
                    Percent = ParsePercent(progressMatch.Groups["percent"].Value),
                    TotalSize = ValueOrNull(progressMatch.Groups["size"])?.Trim(),
                    Speed = ValueOrNull(progressMatch.Groups["speed"])?.Trim(),
                    Eta = ValueOrNull(progressMatch.Groups["eta"]),
                    Message = "Downloading",
                    RawLine = line
                };
            }

            var destinationMatch = DestinationRegex.Match(line);
            if (destinationMatch.Success)
            {
                return new YtDlpProgress
                {
                    Status = line.StartsWith("[ExtractAudio]", StringComparison.OrdinalIgnoreCase)
                        ? YtDlpJobStatus.Converting
                        : YtDlpJobStatus.Downloading,
                    FilePath = TrimQuotes(destinationMatch.Groups["path"].Value),
                    Message = line.StartsWith("[ExtractAudio]", StringComparison.OrdinalIgnoreCase)
                        ? "Extracting audio"
                        : "Downloading",
                    RawLine = line
                };
            }

            var mergerMatch = MergerRegex.Match(line);
            if (mergerMatch.Success)
            {
                return new YtDlpProgress
                {
                    Status = YtDlpJobStatus.Converting,
                    FilePath = TrimQuotes(mergerMatch.Groups["path"].Value),
                    Message = "Merging formats",
                    RawLine = line
                };
            }

            if (line.StartsWith("[info]", StringComparison.OrdinalIgnoreCase) ||
                line.StartsWith("[youtube]", StringComparison.OrdinalIgnoreCase))
            {
                return new YtDlpProgress
                {
                    Status = YtDlpJobStatus.FetchingInfo,
                    Message = line,
                    RawLine = line
                };
            }

            return new YtDlpProgress
            {
                Message = line,
                RawLine = line
            };
        }

        private static double? ParsePercent(string value)
        {
            double parsed;
            if (Double.TryParse(value, NumberStyles.Float, CultureInfo.InvariantCulture, out parsed))
                return parsed;

            return null;
        }

        private static string ValueOrNull(Group group)
        {
            return group != null && group.Success ? group.Value : null;
        }

        private static string TrimQuotes(string value)
        {
            if (String.IsNullOrWhiteSpace(value))
                return value;

            value = value.Trim();
            if (value.Length >= 2 && value[0] == '"' && value[value.Length - 1] == '"')
                return value.Substring(1, value.Length - 2);

            return value;
        }
    }
}
