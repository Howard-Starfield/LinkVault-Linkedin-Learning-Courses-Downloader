using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using LLCD.CourseContent;
using Microsoft.CSharp;
using Newtonsoft.Json;
using Serilog;

namespace LLCD.CourseExtractor
{
    public class Extractor
    {
        public delegate void LinksExtractionEventHandler();
        private readonly Quality _quality;
        private readonly int _delay;
        private string _courseUrl;
        private string _courseSlug;
        private HttpClient _client;
        private CookieContainer _cookieContainer;
        private string _linkedinHomeRaw;
        private bool _isTokenChecked = false;

        public string EnterpriseProfileHash { get; set; }

        public Extractor(string courseUrl, Quality quality, string token, int delay = 0)
            : this(courseUrl, quality, token, CreateDefaultCookieContainer(), delay)
        {
        }

        private Extractor(string courseUrl, Quality quality, string token, CookieContainer cookieContainer, int delay)
            : this(courseUrl, quality, token, new HttpClient(new HttpClientHandler { UseCookies = true, CookieContainer = cookieContainer }), cookieContainer, delay)
        {
        }

        internal Extractor(string courseUrl, Quality quality, string token, HttpClient client, CookieContainer cookieContainer, int delay = 0)
        {
            _courseUrl = courseUrl;
            _quality = quality;
            _delay = delay;
            _cookieContainer = cookieContainer ?? CreateDefaultCookieContainer();
            AddLinkedInTokenCookie(_cookieContainer, token);
            _client = client ?? throw new ArgumentNullException(nameof(client));
            if (!_client.DefaultRequestHeaders.Contains("User-Agent"))
            {
                _client.DefaultRequestHeaders.Add("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:88.0) Gecko/20100101 Firefox/88.0");
            }
        }

        public static string ExtractToken(Browser browser)
        {
            return ExtractTokens(browser).FirstOrDefault();
        }

        public static async Task<string> ExtractValidToken(Browser browser)
        {
            foreach (var token in ExtractTokens(browser))
            {
                var extractor = new Extractor("https://www.linkedin.com/learning", Quality.Low, token);
                try
                {
                    if (await extractor.HasValidToken())
                    {
                        return token;
                    }
                }
                catch (Exception ex)
                {
                    Log.Error(ex, "Extracted browser token failed validation");
                }
            }
            return null;
        }

        private static List<string> ExtractTokens(Browser browser)
        {
            var cookieExtractor = new CookiesExtractor(".www.linkedin.com");
            List<DBCookie> cookies;
            switch (browser)
            {
                case Browser.Chrome:
                    cookies = cookieExtractor.ReadChromeCookies();
                    break;
                case Browser.Firefox:
                    cookies = cookieExtractor.ReadFirefoxCookies();
                    break;
                case Browser.Edge:
                    cookies = cookieExtractor.ReadEdgeCookies();
                    break;
                default:
                    throw new ArgumentException("browser");
            }
            return cookies
                .Where(c => c.Name == "li_at" && !String.IsNullOrWhiteSpace(c.Value))
                .Select(c => c.Value)
                .Distinct()
                .ToList();
        }

        private static CookieContainer CreateDefaultCookieContainer()
        {
            return new CookieContainer();
        }

        private static void AddLinkedInTokenCookie(CookieContainer cookieContainer, string token)
        {
            if (cookieContainer == null || String.IsNullOrWhiteSpace(token))
                return;

            cookieContainer.Add(new Cookie("li_at", token, "/", ".www.linkedin.com"));
        }

        public async Task<Course> GetCourse(IProgress<float> progress = null, bool includeVideoDetails = true)
        {
            if (!HasValidUrl())
            {
                throw new ArgumentException("Invalid Course Url : " + _courseUrl);
            }
            if (!_isTokenChecked && !await HasValidToken())
            {
                throw new ArgumentException("Invalid Token");
            }
            EnterpriseProfileHash = await ExtractEnterpriseProfileHash();
            if (!String.IsNullOrEmpty(EnterpriseProfileHash))
            {
                _client.DefaultRequestHeaders.Add("x-li-identity", EnterpriseProfileHash);
            }
            var courseResponse = await _client.GetAsync($"https://www.linkedin.com/learning-api/detailedCourses?courseSlug={_courseSlug}&fields=chapters,title,exerciseFiles&addParagraphsToTranscript=true&q=slugs");
            var courseResponseText = await courseResponse.Content.ReadAsStringAsync();

            Course course;
            try
            {
                course = Course.FromJson(courseResponseText);
            }
            catch (Exception ex)
            {
                if (courseResponseText.Contains("CSRF check failed"))
                {
                    throw new ArgumentException("Token is expired. Please use a new one.", ex);
                }
                else
                {
                    Log.Error("Course Deserialization failed. \nResponse text : " + courseResponseText);
                    throw;
                }
            }

            course.Slug = _courseSlug;
            await RefreshExerciseFileUrls(course);
            if (!includeVideoDetails)
            {
                progress?.Report(1);
                return course;
            }

            float j = 1;
            float totalCount = course.Chapters.SelectMany(c => c.Videos).Count();
            foreach (var chapter in course.Chapters)
            {
                for (int i = 0; i < chapter.Videos.Count(); i++, j++)
                {
                    var video = chapter.Videos[i];
                    string slug = video.Slug;
                    video = await GetVideoWithFallback(slug);
                    chapter.Videos[i] = video;
                    progress?.Report(j / totalCount);
                    await Task.Delay(_delay * 1000);
                }
            }
            return course;
        }

        private async Task<Video> GetVideoWithFallback(string slug)
        {
            string lastResponseText = null;
            Exception lastDeserializationException = null;

            foreach (var height in _quality.ToHeights())
            {
                var videoResponse = await _client.GetAsync($"https://www.linkedin.com/learning-api/detailedCourses?courseSlug={_courseSlug}&resolution=_{height}&q=slugs&fields=selectedVideo&videoSlug={slug}");
                lastResponseText = await videoResponse.Content.ReadAsStringAsync();
                Video video;
                try
                {
                    video = Video.FromJson(lastResponseText);
                }
                catch (Exception ex)
                {
                    lastDeserializationException = ex;
                    Log.Warning(ex, "Video deserialization failed for requested LinkedIn resolution {Resolution}", height);
                    continue;
                }

                if (video == null)
                {
                    Log.Warning("LinkedIn returned an empty selected video response for requested resolution {Resolution}", height);
                    continue;
                }

                video.Slug = slug;
                if (!String.IsNullOrWhiteSpace(video.DownloadUrl))
                {
                    return video;
                }

                Log.Information("LinkedIn video resolution {Resolution} was not available for {VideoSlug}; trying fallback", height, slug);
            }

            if (lastDeserializationException != null)
            {
                Log.Error(lastDeserializationException, "Video Deserialization failed. \nResponse text : " + lastResponseText);
                throw lastDeserializationException;
            }

            throw new ArgumentException("Failed to extract a course video. The provided token is probably invalid or no requested resolution is available");
        }

        public async Task<bool> HasValidToken()
        {
            if (_linkedinHomeRaw is null)
            {
                var response = await _client.GetAsync("https://www.linkedin.com/learning");
                _linkedinHomeRaw = await response.Content.ReadAsStringAsync();
                _linkedinHomeRaw = WebUtility.HtmlDecode(_linkedinHomeRaw);
            }

            if (HasTrialPrompt(_linkedinHomeRaw))
            {
                return false;
            }
            var cookies = _cookieContainer.GetCookies(new Uri("https://www.linkedin.com/learning"));
            if (cookies["JSESSIONID"] is null)
            {
                return false;
            }
            var jsession = cookies["JSESSIONID"].Value;
            if (_client.DefaultRequestHeaders.Contains("Csrf-Token"))
            {
                _client.DefaultRequestHeaders.Remove("Csrf-Token");
            }
            _client.DefaultRequestHeaders.Add("Csrf-Token", jsession);
            _isTokenChecked = true;
            return true;

        }

        private async Task<String> ExtractEnterpriseProfileHash()
        {
            if (_linkedinHomeRaw is null)
            {
                var response = await _client.GetAsync("https://www.linkedin.com/learning");
                _linkedinHomeRaw = await response.Content.ReadAsStringAsync();
                _linkedinHomeRaw = WebUtility.HtmlDecode(_linkedinHomeRaw);
            }

            return ExtractEnterpriseProfileHash(_linkedinHomeRaw);
        }

        private async Task RefreshExerciseFileUrls(Course course)
        {
            if (course?.ExerciseFiles == null || course.ExerciseFiles.Count == 0)
                return;

            try
            {
                var response = await _client.GetAsync($"https://www.linkedin.com/learning/{_courseSlug}");
                var coursePageHtml = WebUtility.HtmlDecode(await response.Content.ReadAsStringAsync());
                var urls = ExtractExerciseFileUrlsFromHtml(coursePageHtml);
                if (urls.Count == 0)
                    return;

                var matchedUrls = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                var unmatchedExerciseFiles = new List<ExerciseFile>();
                foreach (var exerciseFile in course.ExerciseFiles)
                {
                    var refreshedUrl = FindExerciseFileUrlByName(urls, exerciseFile.FileName);
                    if (!String.IsNullOrWhiteSpace(refreshedUrl))
                    {
                        exerciseFile.DownloadUrl = refreshedUrl;
                        matchedUrls.Add(refreshedUrl);
                    }
                    else
                    {
                        unmatchedExerciseFiles.Add(exerciseFile);
                    }
                }

                var unmatchedUrls = urls
                    .Where(url => !matchedUrls.Contains(url))
                    .ToList();
                if (unmatchedExerciseFiles.Count > 0 && unmatchedExerciseFiles.Count == unmatchedUrls.Count)
                {
                    for (int i = 0; i < unmatchedExerciseFiles.Count; i++)
                    {
                        unmatchedExerciseFiles[i].DownloadUrl = unmatchedUrls[i];
                    }
                }
            }
            catch (Exception ex)
            {
                Log.Warning(ex, "Could not refresh exercise file URLs from course page");
            }
        }

        internal static List<string> ExtractExerciseFileUrlsFromHtml(string html)
        {
            if (String.IsNullOrWhiteSpace(html))
                return new List<string>();

            var normalizedHtml = WebUtility.HtmlDecode(WebUtility.HtmlDecode(html)
                .Replace("\\u002F", "/")
                .Replace("\\u002f", "/")
                .Replace("\\/", "/")
                .Replace("\\u0026", "&")
                .Replace("\\u0026amp;", "&")
                .Replace("\\u003D", "=")
                .Replace("\\u003d", "="));

            var fileUrlMatches = Regex.Matches(
                normalizedHtml,
                @"https?:\/\/[^""'<>\s\\]+(?:\.(?:zip|pdf|rar|7z|tar|gz|docx?|xlsx?|pptx?))(?:\?[^""'<>\s\\]*)?",
                RegexOptions.IgnoreCase);
            var ambryUrlMatches = Regex.Matches(
                normalizedHtml,
                @"https?:\/\/(?:www\.)?linkedin\.com\/ambry\/\?[^""'<>\s\\]+",
                RegexOptions.IgnoreCase);

            return fileUrlMatches
                .Cast<Match>()
                .Concat(ambryUrlMatches.Cast<Match>())
                .Select(match => match.Value.Trim())
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .ToList();
        }

        private static string FindExerciseFileUrlByName(IEnumerable<string> urls, string fileName)
        {
            if (urls == null || String.IsNullOrWhiteSpace(fileName))
                return null;

            foreach (var url in urls)
            {
                string urlFileName = GetUrlFileName(url);
                if (String.Equals(urlFileName, fileName, StringComparison.OrdinalIgnoreCase))
                {
                    return url;
                }
            }

            return null;
        }

        private static string GetUrlFileName(string url)
        {
            if (String.IsNullOrWhiteSpace(url))
                return null;

            try
            {
                var uri = new Uri(url);
                return Path.GetFileName(uri.AbsolutePath);
            }
            catch (UriFormatException)
            {
                return null;
            }
        }

        internal static bool HasTrialPrompt(string html)
        {
            if (String.IsNullOrWhiteSpace(html))
                return false;

            return Regex.IsMatch(html, @"nav__button-tertiary.*Start free trial", RegexOptions.IgnoreCase | RegexOptions.Singleline);
        }

        internal static string ExtractEnterpriseProfileHash(string html)
        {
            if (String.IsNullOrWhiteSpace(html))
                return null;

            var match = Regex.Match(html, @"enterpriseProfileHash"":""(?<enterpriseProfileHash>.*?)""");
            return match.Success ? match.Groups["enterpriseProfileHash"].Value : null;
        }
        public bool HasValidUrl()
        {
            if (String.IsNullOrWhiteSpace(_courseUrl))
                return false;

            _courseUrl = _courseUrl.Trim();
            if (!_courseUrl.StartsWith("https://", StringComparison.OrdinalIgnoreCase) && !_courseUrl.StartsWith("http://", StringComparison.OrdinalIgnoreCase))
            {
                _courseUrl = "https://" + _courseUrl;
            }
            Regex patternCourseUrl = new Regex(@"^https?:\/\/(?:www\.)?linkedin\.com\/learning\/(?<courseSlug>[a-zA-Z0-9-]+)(?:[\/?#]|$)", RegexOptions.IgnoreCase);

            if (patternCourseUrl.IsMatch(_courseUrl))
            {
                _courseSlug = patternCourseUrl.Match(_courseUrl).Groups["courseSlug"].Value;
                return true;
            }
            return false;
        }

    }
}
