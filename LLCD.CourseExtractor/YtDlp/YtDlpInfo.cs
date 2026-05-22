using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using Newtonsoft.Json.Linq;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpInfo
    {
        public string Title { get; set; }

        public string Thumbnail { get; set; }

        public double? Duration { get; set; }

        public string Uploader { get; set; }

        public string Url { get; set; }

        public List<YtDlpFormat> Formats { get; set; } = new List<YtDlpFormat>();

        public List<YtDlpSubtitleTrack> Subtitles { get; set; } = new List<YtDlpSubtitleTrack>();

        public List<YtDlpSubtitleTrack> AutomaticCaptions { get; set; } = new List<YtDlpSubtitleTrack>();

        public static YtDlpInfo FromJson(string json)
        {
            if (String.IsNullOrWhiteSpace(json))
                throw new ArgumentException("yt-dlp metadata JSON is empty.", nameof(json));

            var root = JObject.Parse(json);
            var info = new YtDlpInfo
            {
                Title = Value<string>(root, "title"),
                Thumbnail = Value<string>(root, "thumbnail"),
                Duration = Value<double?>(root, "duration"),
                Uploader = Value<string>(root, "uploader") ?? Value<string>(root, "channel"),
                Url = Value<string>(root, "webpage_url") ?? Value<string>(root, "original_url")
            };

            info.Formats = ExtractFormats(root["formats"] as JArray);
            info.Subtitles = ExtractSubtitleTracks(root["subtitles"] as JObject, false);
            info.AutomaticCaptions = ExtractSubtitleTracks(root["automatic_captions"] as JObject, true);
            return info;
        }

        private static List<YtDlpFormat> ExtractFormats(JArray formats)
        {
            if (formats is null)
                return new List<YtDlpFormat>();

            return formats
                .OfType<JObject>()
                .Select(ToFormat)
                .Where(format => format != null && format.Height.HasValue && !String.IsNullOrWhiteSpace(format.Id))
                .GroupBy(format => format.Height.Value)
                .Select(group => group
                    .OrderByDescending(format => format.Bitrate ?? 0)
                    .ThenBy(format => format.Id, StringComparer.OrdinalIgnoreCase)
                    .First())
                .OrderByDescending(format => format.Height.Value)
                .ToList();
        }

        private static YtDlpFormat ToFormat(JObject item)
        {
            string videoCodec = Value<string>(item, "vcodec");
            if (String.Equals(videoCodec, "none", StringComparison.OrdinalIgnoreCase))
                return null;

            int? height = Value<int?>(item, "height");
            if (!height.HasValue)
                return null;

            return new YtDlpFormat
            {
                Id = Value<string>(item, "format_id"),
                Label = height.Value.ToString(CultureInfo.InvariantCulture) + "p",
                Height = height,
                Bitrate = Value<double?>(item, "tbr"),
                VideoCodec = videoCodec,
                AudioCodec = Value<string>(item, "acodec")
            };
        }

        private static List<YtDlpSubtitleTrack> ExtractSubtitleTracks(JObject subtitles, bool isAutomatic)
        {
            if (subtitles is null)
                return new List<YtDlpSubtitleTrack>();

            var tracks = new List<YtDlpSubtitleTrack>();
            foreach (var property in subtitles.Properties())
            {
                var entries = property.Value as JArray;
                if (entries is null || entries.Count == 0)
                    continue;

                var first = entries.OfType<JObject>().FirstOrDefault();
                tracks.Add(new YtDlpSubtitleTrack
                {
                    Language = property.Name,
                    Name = Value<string>(first, "name") ?? property.Name,
                    Extension = Value<string>(first, "ext"),
                    IsAutomatic = isAutomatic
                });
            }
            return tracks
                .OrderBy(track => track.Language, StringComparer.OrdinalIgnoreCase)
                .ToList();
        }

        private static T Value<T>(JObject item, string name)
        {
            if (item is null)
                return default(T);

            var token = item[name];
            if (token is null || token.Type == JTokenType.Null)
                return default(T);

            return token.ToObject<T>();
        }
    }
}
