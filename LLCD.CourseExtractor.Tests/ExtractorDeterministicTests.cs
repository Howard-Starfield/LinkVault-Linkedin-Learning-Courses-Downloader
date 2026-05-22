using System;
using System.Collections.Generic;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
using LLCD.CourseContent;
using LLCD.CourseExtractor;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class ExtractorDeterministicTests
    {
        [TestMethod]
        public async Task HasValidToken_WithLinkedInSessionCookieAndSignedInHome_ReturnsTrue()
        {
            var cookies = CreateCookiesWithSession();
            var handler = new StaticHtmlHandler("<html><script>\"enterpriseProfileHash\":\"abc123\"</script></html>");
            var client = new HttpClient(handler);
            var extractor = new Extractor(
                "https://www.linkedin.com/learning/example-course",
                Quality.Low,
                "li-at-token",
                client,
                cookies);

            var isValid = await extractor.HasValidToken();

            Assert.IsTrue(isValid);
            Assert.AreEqual("https://www.linkedin.com/learning", handler.RequestUris[0]);
            Assert.IsTrue(client.DefaultRequestHeaders.Contains("Csrf-Token"));
        }

        [TestMethod]
        public async Task HasValidToken_WithTrialPrompt_ReturnsFalse()
        {
            var cookies = CreateCookiesWithSession();
            var handler = new StaticHtmlHandler("<a class=\"nav__button-tertiary\">\nStart free trial</a>");
            var extractor = new Extractor(
                "https://www.linkedin.com/learning/example-course",
                Quality.Low,
                "li-at-token",
                new HttpClient(handler),
                cookies);

            var isValid = await extractor.HasValidToken();

            Assert.IsFalse(isValid);
        }

        [TestMethod]
        public async Task HasValidToken_WithoutSessionCookie_ReturnsFalse()
        {
            var handler = new StaticHtmlHandler("<html>signed in shell</html>");
            var extractor = new Extractor(
                "https://www.linkedin.com/learning/example-course",
                Quality.Low,
                "li-at-token",
                new HttpClient(handler),
                new CookieContainer());

            var isValid = await extractor.HasValidToken();

            Assert.IsFalse(isValid);
        }

        [TestMethod]
        public void HasTrialPrompt_WithMultilineMixedCasePrompt_ReturnsTrue()
        {
            var html = "<a class=\"NAV__BUTTON-TERTIARY other-class\">\r\nStart free trial</a>";

            Assert.IsTrue(Extractor.HasTrialPrompt(html));
        }

        [TestMethod]
        public void ExtractEnterpriseProfileHash_WithHashInLinkedInBootstrapHtml_ReturnsHash()
        {
            var html = "<script>{\"enterpriseProfileHash\":\"urn-li-enterprise-profile\"}</script>";

            var enterpriseProfileHash = Extractor.ExtractEnterpriseProfileHash(html);

            Assert.AreEqual("urn-li-enterprise-profile", enterpriseProfileHash);
        }

        [TestMethod]
        public void ExtractEnterpriseProfileHash_WithoutHash_ReturnsNull()
        {
            Assert.IsNull(Extractor.ExtractEnterpriseProfileHash("<html></html>"));
        }

        [TestMethod]
        public void HasValidUrl_WithBareLinkedInLearningUrl_ReturnsTrue()
        {
            var extractor = new Extractor("www.linkedin.com/learning/example-course/welcome?u=123", Quality.Low, string.Empty);

            Assert.IsTrue(extractor.HasValidUrl());
        }

        [TestMethod]
        public void HasValidUrl_WithEmbeddedLinkedInUrlInDifferentHost_ReturnsFalse()
        {
            var extractor = new Extractor("https://example.com/?next=https://www.linkedin.com/learning/example-course", Quality.Low, string.Empty);

            Assert.IsFalse(extractor.HasValidUrl());
        }

        [TestMethod]
        public void HasValidUrl_WithBlankUrl_ReturnsFalse()
        {
            var extractor = new Extractor("   ", Quality.Low, string.Empty);

            Assert.IsFalse(extractor.HasValidUrl());
        }

        [TestMethod]
        public void ExtractExerciseFileUrlsFromHtml_WithEscapedCoursePageUrls_ReturnsNormalizedUrls()
        {
            var html = @"{""url"":""https:\/\/files3.lynda.com\/secure\/courses\/123\/exercises\/exercise.zip?token=a\u0026b=c""}";

            var urls = Extractor.ExtractExerciseFileUrlsFromHtml(html);

            Assert.AreEqual(1, urls.Count);
            Assert.AreEqual("https://files3.lynda.com/secure/courses/123/exercises/exercise.zip?token=a&b=c", urls[0]);
        }

        [TestMethod]
        public void ExtractExerciseFileUrlsFromHtml_WithEscapedAmbryUrl_ReturnsNormalizedUrl()
        {
            var html = @"{""url"":""https:\/\/www.linkedin.com\/ambry\/?x-li-ambry-ep=AQK123\u0026amp;download=true""}";

            var urls = Extractor.ExtractExerciseFileUrlsFromHtml(html);

            Assert.AreEqual(1, urls.Count);
            Assert.AreEqual("https://www.linkedin.com/ambry/?x-li-ambry-ep=AQK123&download=true", urls[0]);
        }

        [TestMethod]
        public async Task GetCourse_WithFakeLinkedInApiResponse_BuildsCourseAndTranscripts()
        {
            var cookies = CreateCookiesWithSession();
            var handler = new LinkedInCourseApiHandler();
            var extractor = new Extractor(
                "https://www.linkedin.com/learning/sample-course/welcome",
                Quality.Low,
                "li-at-token",
                new HttpClient(handler),
                cookies);
            var progress = new CollectingProgress();

            var course = await extractor.GetCourse(progress);

            Assert.AreEqual("Sample Course", course.Title);
            Assert.AreEqual("sample-course", course.Slug);
            Assert.AreEqual(1, course.ExerciseFiles.Count);
            Assert.AreEqual("exercise.zip", course.ExerciseFiles[0].FileName);
            Assert.AreEqual("https://www.linkedin.com/ambry/?x-li-ambry-ep=AQK123&download=true", course.ExerciseFiles[0].DownloadUrl);
            Assert.AreEqual(1, course.Chapters.Count);
            Assert.AreEqual("Getting started", course.Chapters[0].Title);
            Assert.AreEqual(1, course.Chapters[0].Videos.Count);
            Assert.AreEqual("welcome", course.Chapters[0].Videos[0].Slug);
            Assert.AreEqual("Welcome video", course.Chapters[0].Videos[0].Title);
            Assert.AreEqual("https://cdn.example.test/welcome.mp4", course.Chapters[0].Videos[0].DownloadUrl);
            StringAssert.Contains(course.Chapters[0].Videos[0].Transcript, "00:00:00,000 --> 00:00:01,500");
            StringAssert.Contains(course.Chapters[0].Videos[0].Transcript, "Hello there");
            StringAssert.Contains(course.Chapters[0].Videos[0].Transcript, "00:00:01,500 --> 00:00:03,000");
            StringAssert.Contains(course.Chapters[0].Videos[0].Transcript, "Welcome back");
            Assert.AreEqual(1, progress.Values.Count);
            Assert.AreEqual(1.0f, progress.Values[0]);
            Assert.IsTrue(handler.RequestUris.Any(uri => uri.Contains("courseSlug=sample-course")));
            Assert.IsTrue(handler.RequestUris.Any(uri => uri.Contains("resolution=_360")));
            Assert.AreEqual("urn-li-enterprise-profile", handler.LastIdentityHeader);
        }

        [TestMethod]
        public async Task GetCourse_WithVideoDetailsDisabled_SkipsSelectedVideoRequests()
        {
            var cookies = CreateCookiesWithSession();
            var handler = new LinkedInCourseApiHandler();
            var extractor = new Extractor(
                "https://www.linkedin.com/learning/sample-course/welcome",
                Quality.Low,
                "li-at-token",
                new HttpClient(handler),
                cookies);
            var progress = new CollectingProgress();

            var course = await extractor.GetCourse(progress, includeVideoDetails: false);

            Assert.AreEqual("Sample Course", course.Title);
            Assert.AreEqual(1, course.Chapters.Count);
            Assert.AreEqual(1, course.Chapters[0].Videos.Count);
            Assert.IsFalse(handler.RequestUris.Any(uri => uri.Contains("fields=selectedVideo")));
            Assert.IsFalse(handler.RequestUris.Any(uri => uri.Contains("resolution=_360")));
            Assert.AreEqual(1.0f, progress.Values.Last());
        }

        private static CookieContainer CreateCookiesWithSession()
        {
            var cookies = new CookieContainer();
            cookies.Add(new Cookie("JSESSIONID", "ajax:123", "/", ".www.linkedin.com"));
            return cookies;
        }

        private class StaticHtmlHandler : HttpMessageHandler
        {
            private readonly string _html;

            public StaticHtmlHandler(string html)
            {
                _html = html;
                RequestUris = new List<string>();
            }

            public List<string> RequestUris { get; }

            protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
            {
                RequestUris.Add(request.RequestUri.ToString());
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
                {
                    Content = new StringContent(_html)
                });
            }
        }

        private class CollectingProgress : IProgress<float>
        {
            public CollectingProgress()
            {
                Values = new List<float>();
            }

            public List<float> Values { get; }

            public void Report(float value)
            {
                Values.Add(value);
            }
        }

        private class LinkedInCourseApiHandler : HttpMessageHandler
        {
            public LinkedInCourseApiHandler()
            {
                RequestUris = new List<string>();
            }

            public List<string> RequestUris { get; }

            public string LastIdentityHeader { get; private set; }

            protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
            {
                string requestUri = request.RequestUri.ToString();
                RequestUris.Add(requestUri);
                if (request.Headers.TryGetValues("x-li-identity", out var identityValues))
                {
                    LastIdentityHeader = identityValues.FirstOrDefault();
                }

                if (requestUri == "https://www.linkedin.com/learning")
                {
                    return Response("<html><script>{\"enterpriseProfileHash\":\"urn-li-enterprise-profile\"}</script></html>");
                }

                if (requestUri == "https://www.linkedin.com/learning/sample-course")
                {
                    return Response(@"<html><script>{
                        ""exerciseFiles"": [{
                            ""name"": ""exercise.zip"",
                            ""url"": ""https:\/\/www.linkedin.com\/ambry\/?x-li-ambry-ep=AQK123\u0026download=true""
                        }]
                    }</script></html>");
                }

                if (requestUri.Contains("fields=chapters,title,exerciseFiles"))
                {
                    return Response(@"{
                        ""elements"": [{
                            ""title"": ""Sample Course"",
                            ""exerciseFiles"": [{
                                ""name"": ""exercise.zip"",
                                ""url"": ""https://cdn.example.test/exercise.zip""
                            }],
                            ""chapters"": [{
                                ""title"": ""Getting started"",
                                ""videos"": [{
                                    ""slug"": ""welcome""
                                }]
                            }]
                        }]
                    }");
                }

                if (requestUri.Contains("fields=selectedVideo") && requestUri.Contains("videoSlug=welcome"))
                {
                    return Response(@"{
                        ""elements"": [{
                            ""selectedVideo"": {
                                ""title"": ""Welcome video"",
                                ""durationInSeconds"": 3,
                                ""url"": {
                                    ""progressiveUrl"": ""https://cdn.example.test/welcome.mp4""
                                },
                                ""transcript"": {
                                    ""lines"": [{
                                        ""caption"": ""Hello there"",
                                        ""transcriptStartAt"": 0
                                    }, {
                                        ""caption"": ""Welcome back"",
                                        ""transcriptStartAt"": 1500
                                    }]
                                }
                            }
                        }]
                    }");
                }

                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound)
                {
                    Content = new StringContent("Unexpected URL: " + requestUri)
                });
            }

            private static Task<HttpResponseMessage> Response(string body)
            {
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
                {
                    Content = new StringContent(body)
                });
            }
        }
    }
}
