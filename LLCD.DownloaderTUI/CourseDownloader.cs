using LLCD.CourseContent;
using LLCD.CourseExtractor;
using Serilog;
using ShellProgressBar;
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Threading.Tasks;

namespace LLCD.DownloaderTUI
{
    public static class CourseDownloader
    {
        private static ChildProgressBar pbarVideo;
        private static ChildProgressBar pbarExerciseFiles;
        private static string currentVideo;
        private static string currentExerciseFile;
        private static int currentIndex;
        private static readonly HttpClient downloadClient = new HttpClient
        {
            Timeout = TimeSpan.FromMinutes(30)
        };
        #region ProgressBarOptions


        private readonly static ProgressBarOptions optionsChapter = new ProgressBarOptions
        {
            ScrollChildrenIntoView = true,
            ForegroundColor = ConsoleColor.Blue,
            ForegroundColorDone = ConsoleColor.DarkGreen,
            BackgroundColor = ConsoleColor.Gray,
            ProgressBarOnBottom = true,
            CollapseWhenFinished = false
        };

        private readonly static ProgressBarOptions optionsVideo = new ProgressBarOptions
        {
            ScrollChildrenIntoView = true,
            ForegroundColor = ConsoleColor.Yellow,
            ForegroundColorDone = ConsoleColor.DarkGreen,
            BackgroundColor = ConsoleColor.DarkGray,
            ProgressCharacter = '\u2593',
            ProgressBarOnBottom = true,
            CollapseWhenFinished = true
        };
        private readonly static ProgressBarOptions optionsCourse = new ProgressBarOptions
        {
            ScrollChildrenIntoView = true,
            ForegroundColor = ConsoleColor.DarkGray,
            ForegroundColorDone = ConsoleColor.DarkGreen,
            BackgroundColor = ConsoleColor.White,
            ProgressBarOnBottom = true,
            CollapseWhenFinished = false
        };
        #endregion
        private static string ToSafeFileName(string fileName) => string.Concat(fileName.Split(Path.GetInvalidFileNameChars()));
        public static void DownloadCourse(Course course, DirectoryInfo courseRootDirectory, bool downloadVideos = true)
        {
            try
            {
                int exerciseFilesCount = course.ExerciseFiles is null ? 0 : course.ExerciseFiles.Count;
                using var pbarCourse = new ProgressBar(course.Chapters.ToList().Count + +exerciseFilesCount, "Downloading Course : " + course.Title, optionsCourse);
                var courseDirectory = courseRootDirectory.CreateSubdirectory(ToSafeFileName(course.Title));

                if (course.ExerciseFiles != null && course.ExerciseFiles.Count > 0)
                {
                    var failedExerciseFiles = new List<string>();
                    foreach (var exerciseFile in course.ExerciseFiles)
                    {
                        if (!TryDownloadExerciseFile(courseDirectory, pbarCourse, exerciseFile, out var failure))
                        {
                            failedExerciseFiles.Add(failure);
                        }
                        pbarCourse.Tick();
                    }
                    if (failedExerciseFiles.Count > 0)
                    {
                        SaveExerciseFileFailureReport(courseDirectory, failedExerciseFiles);
                    }
                }

                for (int i = 0; i < course.Chapters.Count; i++)
                {
                    var chapter = course.Chapters[i];
                    var chapterDirectory = courseDirectory.CreateSubdirectory($"{(i + 1):D2} - {ToSafeFileName(chapter.Title)}");
                    using var pbarChapter = pbarCourse.Spawn(chapter.Videos.ToList().Count, $"Downloading Chapter {i + 1} : {chapter.Title}", optionsChapter);
                    if (!downloadVideos)
                    {
                        pbarChapter.Message = $"Chapter {i + 1} : {chapter.Title} video downloads were skipped";
                        pbarCourse.Tick();
                        continue;
                    }

                    for (int j = 0; j < chapter.Videos.Count; j++)
                    {
                        var video = chapter.Videos[j];
                        currentVideo = video.Title;
                        currentIndex = j + 1;
                        DownloadVideo(chapterDirectory, pbarChapter, video);
                        pbarChapter.Tick();
                    }
                    pbarChapter.Message = $"Chapter {i + 1} : {chapter.Title} chapter has been downloaded successfully";
                    pbarCourse.Tick();
                }
                pbarCourse.Message = $"{course.Title} course has been downloaded successfully";

                Console.WriteLine();
                Console.ForegroundColor = ConsoleColor.Green;
                Console.WriteLine("Course downloaded successfully :)");
                Console.ResetColor();
                Log.Information("Course downloaded successfully");
            }
            catch (Exception ex)
            {
                TUI.ShowError("An error occured while downloading the course");
                TUI.ShowError("Error details : " + ex.Message);
                Log.Error(ex, "Error while Downloading");
                throw;
            }
        }

        private static void DownloadVideo(DirectoryInfo chapterDirectory, ChildProgressBar pbarChapter, Video video)
        {
            using (pbarVideo = pbarChapter.Spawn(100, $"Downloading Video {currentIndex} : {currentVideo}", optionsVideo))
            {
                Retry.Do(() =>
                {
                    string videoName = $"{currentIndex:D2} - { ToSafeFileName(video.Title)}.mp4";
                    if (!String.IsNullOrWhiteSpace(video.Transcript))
                    {
                        string captionName = $"{currentIndex:D2} - { ToSafeFileName(video.Title)}.srt";
                        File.WriteAllText($"{Path.Combine(chapterDirectory.FullName, ToSafeFileName(captionName))}", video.Transcript);
                    }
                    DownloadFileAsync(new Uri(video.DownloadUrl), Path.Combine(chapterDirectory.FullName, videoName), VideoDownloadProgressChanged).GetAwaiter().GetResult();
                    VideoDownloadCompleted();
                },
                exceptionMessage: "Failed to download video with title " + video.Title,
                actionOnError: () =>
                {
                    var progress = pbarVideo.AsProgress<float>();
                    progress?.Report(0);
                });
            }

        }

        private static void VideoDownloadCompleted()
        {
            pbarVideo.Message = $"Video {currentIndex} : {currentVideo} has been downloaded successfully";
            pbarVideo.AsProgress<float>().Report(1);
        }

        private static void VideoDownloadProgressChanged(long bytesReceived, long totalBytes)
        {
            float KbReceived = bytesReceived / 1024f;
            if (totalBytes <= 0)
            {
                pbarVideo.Message = $"Downloading Video {currentIndex} : {currentVideo} {KbReceived:f0}KB";
                return;
            }

            float TotalKbToReceive = totalBytes / 1024f;
            pbarVideo.Message = $"Downloading Video {currentIndex} : {currentVideo} {KbReceived:f0}KB out of {TotalKbToReceive:f0}KB";
            pbarVideo.AsProgress<float>()?.Report(KbReceived / TotalKbToReceive);
        }

        private static void DownloadExerciseFile(DirectoryInfo courseDirectory, ProgressBar pbarCourse, ExerciseFile exerciseFile)
        {
            using (pbarExerciseFiles = pbarCourse.Spawn(100, $"Downloading Exercise File : {exerciseFile.FileName}", optionsVideo))
            {
                Retry.Do(() =>
                {
                    currentExerciseFile = exerciseFile.FileName;
                    string exerciseFilePath = Path.Combine(courseDirectory.FullName, ToSafeFileName(exerciseFile.FileName));
                    DownloadFileAsync(new Uri(exerciseFile.DownloadUrl), exerciseFilePath, ExerciseFileDownloadProgressChanged).GetAwaiter().GetResult();
                    ExtractExerciseFileArchive(exerciseFilePath);
                    ExerciseFileDownloadCompleted();
                },
                exceptionMessage: "Failed to download exerciseFile with name " + exerciseFile.FileName,
                actionOnError: () =>
                {
                    var progress = pbarExerciseFiles.AsProgress<float>();
                    progress?.Report(0);
                });
            }

        }

        private static void ExtractExerciseFileArchive(string exerciseFilePath)
        {
            var extractionResult = ExerciseArchiveExtractor.ExtractZipAndDeleteArchive(exerciseFilePath);
            if (!extractionResult.Attempted)
                return;

            if (!extractionResult.Succeeded)
            {
                throw new InvalidDataException("Downloaded exercise zip could not be extracted: " + extractionResult.Message);
            }

            if (!String.IsNullOrWhiteSpace(extractionResult.Message))
            {
                Log.Warning(extractionResult.Message);
            }
            currentExerciseFile = Path.GetFileName(extractionResult.DestinationDirectory);
        }

        private static bool TryDownloadExerciseFile(DirectoryInfo courseDirectory, ProgressBar pbarCourse, ExerciseFile exerciseFile, out string failure)
        {
            try
            {
                DownloadExerciseFile(courseDirectory, pbarCourse, exerciseFile);
                failure = null;
                return true;
            }
            catch (Exception ex)
            {
                failure = $"{exerciseFile.FileName}: {ex.Message}";
                TUI.ShowError("Skipping exercise file after retries: " + exerciseFile.FileName);
                Log.Error(ex, "Skipping exercise file after retry exhaustion: {exerciseFile}", exerciseFile.FileName);
                return false;
            }
        }

        private static void ExerciseFileDownloadCompleted()
        {
            pbarExerciseFiles.Message = $"Exercise File {currentExerciseFile} has been downloaded successfully";
            pbarExerciseFiles.AsProgress<float>().Report(1);
        }

        private static void ExerciseFileDownloadProgressChanged(long bytesReceived, long totalBytes)
        {
            float mbReceived = bytesReceived / 1024f / 1024f;
            if (totalBytes <= 0)
            {
                pbarExerciseFiles.Message = $"Downloading Exercise File {currentExerciseFile} {mbReceived:f2}MB";
                return;
            }

            float TotalmbToReceive = totalBytes / 1024f / 1024f;
            pbarExerciseFiles.Message = $"Downloading Exercise File {currentExerciseFile} {mbReceived:f2}MB out of {TotalmbToReceive:f2}MB";
            pbarExerciseFiles.AsProgress<float>()?.Report(mbReceived / TotalmbToReceive);
        }

        private static async Task DownloadFileAsync(Uri uri, string filePath, Action<long, long> progressCallback)
        {
            using var response = await downloadClient.GetAsync(uri, HttpCompletionOption.ResponseHeadersRead).ConfigureAwait(false);
            response.EnsureSuccessStatusCode();

            long length = response.Content.Headers.ContentLength ?? -1;
            if (IsExistingDownloadComplete(filePath, length))
            {
                progressCallback(length, length);
                return;
            }

            string tempFilePath = filePath + ".download";
            if (File.Exists(tempFilePath))
            {
                File.Delete(tempFilePath);
            }

            using var stream = await response.Content.ReadAsStreamAsync().ConfigureAwait(false);
            using var fileStream = File.Create(tempFilePath);

            byte[] buffer = new byte[16384];
            long totalRead = 0;
            int read;
            while ((read = await stream.ReadAsync(buffer, 0, buffer.Length).ConfigureAwait(false)) > 0)
            {
                await fileStream.WriteAsync(buffer, 0, read).ConfigureAwait(false);
                totalRead += read;
                progressCallback(totalRead, length);
            }

            if (length != -1 && totalRead != length)
            {
                throw new IOException($"Incomplete download. Expected {length} bytes but received {totalRead} bytes.");
            }

            fileStream.Close();
            if (File.Exists(filePath))
            {
                File.Delete(filePath);
            }
            File.Move(tempFilePath, filePath);
        }

        private static bool IsExistingDownloadComplete(string filePath, long expectedBytes)
        {
            if (expectedBytes <= 0 || !File.Exists(filePath))
                return false;

            return new FileInfo(filePath).Length == expectedBytes;
        }

        private static void SaveExerciseFileFailureReport(DirectoryInfo courseDirectory, IEnumerable<string> failures)
        {
            string reportPath = Path.Combine(courseDirectory.FullName, "exercise-file-download-failures.txt");
            using (var streamWriter = new StreamWriter(reportPath, false))
            {
                streamWriter.WriteLine("Some exercise files could not be downloaded.");
                streamWriter.WriteLine("If the error says the host cannot be found, the CDN host may be blocked by DNS, VPN, firewall, or network filtering.");
                streamWriter.WriteLine("Try a different network, disable filtering/VPN temporarily, or download the exercise files from the LinkedIn Learning course Overview tab.");
                streamWriter.WriteLine();
                foreach (var failure in failures)
                {
                    streamWriter.WriteLine(failure);
                }
            }
        }
    }
}
