using System;
using System.Collections.Generic;
using System.Linq;
using Newtonsoft.Json.Linq;

namespace LLCD.CourseExtractor.YtDlp
{
    public class YtDlpPlaylistInfo
    {
        public string Title { get; set; }

        public string Url { get; set; }

        public List<YtDlpPlaylistEntry> Entries { get; set; } = new List<YtDlpPlaylistEntry>();

        public bool IsPlaylist => Entries.Count > 0;

        public static YtDlpPlaylistInfo FromJson(string json)
        {
            if (String.IsNullOrWhiteSpace(json))
                throw new ArgumentException("yt-dlp playlist metadata JSON is empty.", nameof(json));

            var root = JObject.Parse(json);
            var info = new YtDlpPlaylistInfo
            {
                Title = Value<string>(root, "title"),
                Url = Value<string>(root, "webpage_url") ?? Value<string>(root, "original_url")
            };

            var entries = root["entries"] as JArray;
            if (entries != null)
            {
                info.Entries = entries
                    .OfType<JObject>()
                    .Select(ToEntry)
                    .Where(entry => entry != null && !String.IsNullOrWhiteSpace(entry.Url))
                    .ToList();
            }
            else
            {
                var single = ToEntry(root);
                if (single != null && !String.IsNullOrWhiteSpace(single.Url))
                {
                    single.Index = 1;
                    info.Entries.Add(single);
                }
            }

            return info;
        }

        private static YtDlpPlaylistEntry ToEntry(JObject item)
        {
            if (item is null)
                return null;

            var webpageUrl = Value<string>(item, "webpage_url") ?? Value<string>(item, "original_url");
            var url = webpageUrl ?? Value<string>(item, "url");

            return new YtDlpPlaylistEntry
            {
                Index = Value<int?>(item, "playlist_index"),
                Id = Value<string>(item, "id"),
                Title = Value<string>(item, "title"),
                Url = url,
                WebpageUrl = webpageUrl
            };
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
