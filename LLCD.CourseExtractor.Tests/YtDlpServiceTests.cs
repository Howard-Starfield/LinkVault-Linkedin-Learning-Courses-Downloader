using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using LLCD.CourseExtractor.YtDlp;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace LLCD.CourseExtractor.Tests
{
    [TestClass]
    public class YtDlpServiceTests
    {
        [TestMethod]
        public async Task GetInfo_UsesInjectedRunnerAndBrowserCookies()
        {
            var runner = new FakeYtDlpProcessRunner
            {
                Result = new YtDlpProcessResult
                {
                    ExitCode = 0,
                    StandardOutput = @"{ ""title"": ""Runner Video"", ""webpage_url"": ""https://example.com/watch"" }"
                }
            };
            var service = new YtDlpService("fake-yt-dlp", runner);

            var info = await service.GetInfo("https://example.com/video", YtDlpBrowserCookiesSource.Chrome);

            Assert.AreEqual("Runner Video", info.Title);
            Assert.AreEqual("fake-yt-dlp", runner.LastCommand.ExecutablePath);
            CollectionAssert.AreEqual(new[]
            {
                "-J",
                "--no-playlist",
                "--cookies-from-browser",
                "chrome",
                "https://example.com/video"
            }, runner.LastCommand.Arguments.ToArray());
        }

        [TestMethod]
        public async Task GetInfo_WhenTokenIsCancelledAfterRunnerReturns_ThrowsOperationCanceled()
        {
            using (var cancellationTokenSource = new CancellationTokenSource())
            {
                var runner = new FakeYtDlpProcessRunner
                {
                    Result = new YtDlpProcessResult
                    {
                        ExitCode = 1,
                        StandardError = "killed"
                    },
                    BeforeReturn = () => cancellationTokenSource.Cancel()
                };
                var service = new YtDlpService("fake-yt-dlp", runner);

                try
                {
                    await service.GetInfo("https://example.com/video", cancellationToken: cancellationTokenSource.Token);
                    Assert.Fail("Expected OperationCanceledException.");
                }
                catch (OperationCanceledException)
                {
                }
            }
        }

        [TestMethod]
        public async Task GetPlaylistInfo_ForwardsCancellationTokenToRunner()
        {
            var runner = new FakeYtDlpProcessRunner
            {
                Result = new YtDlpProcessResult
                {
                    ExitCode = 0,
                    StandardOutput = @"{ ""title"": ""Playlist"", ""entries"": [] }"
                }
            };
            var service = new YtDlpService("fake-yt-dlp", runner);

            using (var cancellationTokenSource = new CancellationTokenSource())
            {
                await service.GetPlaylistInfo("https://example.com/playlist", cancellationToken: cancellationTokenSource.Token);
            }

            Assert.IsTrue(runner.LastCancellationToken.CanBeCanceled);
        }

        [TestMethod]
        public async Task Download_ForwardsRunnerLinesToCallbacks()
        {
            var runner = new FakeYtDlpProcessRunner
            {
                OutputLines = { "[download]  50.0% of 10.00MiB at 1.00MiB/s ETA 00:05" },
                ErrorLines = { "[warning] sample warning" },
                Result = new YtDlpProcessResult { ExitCode = 0 }
            };
            var service = new YtDlpService("fake-yt-dlp", runner);
            var output = new List<string>();
            var errors = new List<string>();

            await service.Download(new YtDlpDownloadOptions
            {
                Url = "https://example.com/video",
                OutputTemplate = "%(title)s.%(ext)s"
            }, output.Add, errors.Add);

            Assert.AreEqual(1, output.Count);
            Assert.AreEqual(1, errors.Count);
            Assert.AreEqual("--newline", runner.LastCommand.Arguments[0]);
        }

        [TestMethod]
        public async Task GetPlaylistInfo_DoesNotAddNoPlaylistArgument()
        {
            var runner = new FakeYtDlpProcessRunner
            {
                Result = new YtDlpProcessResult
                {
                    ExitCode = 0,
                    StandardOutput = @"{ ""title"": ""Playlist"", ""entries"": [{ ""title"": ""One"", ""webpage_url"": ""https://example.com/one"" }] }"
                }
            };
            var service = new YtDlpService("fake-yt-dlp", runner);

            var info = await service.GetPlaylistInfo("https://example.com/playlist");

            Assert.AreEqual(1, info.Entries.Count);
            CollectionAssert.DoesNotContain(runner.LastCommand.Arguments.ToArray(), "--no-playlist");
            CollectionAssert.AreEqual(new[]
            {
                "-J",
                "https://example.com/playlist"
            }, runner.LastCommand.Arguments.ToArray());
        }

        [TestMethod]
        public async Task JobRunner_UpdatesJobStatusAndReturnsOutputPath()
        {
            var runner = new FakeYtDlpProcessRunner
            {
                OutputLines =
                {
                    "[download] 100.0% of 10.00MiB at 1.00MiB/s ETA 00:00",
                    @"[Merger] Merging formats into ""D:\Videos\sample.mp4""",
                    "[FixupM3u8] Fixing MPEG-TS in MP4 container of \"D:\\Videos\\sample.mp4\""
                },
                Result = new YtDlpProcessResult { ExitCode = 0 }
            };
            var job = new YtDlpJob
            {
                Options = new YtDlpDownloadOptions
                {
                    Url = "https://example.com/video",
                    OutputTemplate = @"D:\Videos\%(title)s.%(ext)s"
                }
            };

            var jobRunner = new YtDlpJobRunner(new YtDlpService("fake-yt-dlp", runner));
            var result = await jobRunner.Download(job);

            Assert.IsTrue(result.Success);
            Assert.AreEqual(YtDlpJobStatus.Finished, job.Status);
            Assert.AreEqual(@"D:\Videos\sample.mp4", result.FilePath);
            Assert.AreEqual("sample.mp4", result.FileName);
            Assert.AreEqual(@"D:\Videos\sample.mp4", job.OutputFilePath);
            Assert.AreEqual("sample.mp4", job.OutputFileName);
            Assert.AreEqual(3, job.Logs.Count);
        }

        private class FakeYtDlpProcessRunner : IYtDlpProcessRunner
        {
            public FakeYtDlpProcessRunner()
            {
                OutputLines = new List<string>();
                ErrorLines = new List<string>();
                Result = new YtDlpProcessResult { ExitCode = 0 };
            }

            public YtDlpCommand LastCommand { get; private set; }

            public CancellationToken LastCancellationToken { get; private set; }

            public List<string> OutputLines { get; }

            public List<string> ErrorLines { get; }

            public YtDlpProcessResult Result { get; set; }

            public Action BeforeReturn { get; set; }

            public Task<YtDlpProcessResult> RunAsync(YtDlpCommand command, CancellationToken cancellationToken, Action<string> outputLine = null, Action<string> errorLine = null)
            {
                LastCommand = command;
                LastCancellationToken = cancellationToken;
                foreach (var line in OutputLines)
                {
                    outputLine?.Invoke(line);
                }
                foreach (var line in ErrorLines)
                {
                    errorLine?.Invoke(line);
                }

                BeforeReturn?.Invoke();
                return Task.FromResult(Result);
            }
        }
    }
}
