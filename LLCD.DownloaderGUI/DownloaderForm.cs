using LLCD.CourseContent;
using LLCD.CourseExtractor;
using Serilog;
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Data;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace LLCD.DownloaderGUI
{
    public partial class DownloaderForm : Form
    {
        private List<Course> _courses;
        private DirectoryInfo _courseRootDirectory;
        private readonly bool _toDownloadVideos;
        private readonly bool _toDownloadExerciseFiles;
        private readonly bool _toDownloadSubtitles;
        private int _videosCount;
        private int _currentVideoIndex = 1;
        private CancellationTokenSource _cancellationTokenSource = new CancellationTokenSource();
        private CancellationToken _cancellationToken;
        private Downloader _downloader = new Downloader();

        [DesignerSerializationVisibility(DesignerSerializationVisibility.Hidden)]
        public CourseStatus DownloaderStatus { get; set; } = CourseStatus.Running;

        public DownloaderForm(List<Course> courses, DirectoryInfo courseRootDirectory, bool toDownloadVideos, bool toDownloadExerciseFiles, bool toDownloadSubtitles)
        {
            _courses = courses;
            _courseRootDirectory = courseRootDirectory;
            _toDownloadVideos = toDownloadVideos;
            _toDownloadExerciseFiles = toDownloadExerciseFiles;
            _toDownloadSubtitles = toDownloadSubtitles;
            _cancellationToken = _cancellationTokenSource.Token;
            InitializeComponent();
            Text = "Downloading Courses";
            ConfigureResponsiveLayout();
            FormHelpers.SetFonts(flowLayoutPanel);
            UpdateResponsiveLayout();
        }

        private void ConfigureResponsiveLayout()
        {
            var workingArea = Screen.FromControl(this).WorkingArea;
            var desiredClientSize = new Size(700, 320);
            var minimumClientSize = new Size(
                Math.Min(640, Math.Max(520, workingArea.Width - 80)),
                Math.Min(300, Math.Max(260, workingArea.Height - 80)));

            AutoScroll = true;
            ClientSize = new Size(
                Math.Min(desiredClientSize.Width, Math.Max(minimumClientSize.Width, workingArea.Width - 80)),
                Math.Min(desiredClientSize.Height, Math.Max(minimumClientSize.Height, workingArea.Height - 80)));
            MinimumSize = SizeFromClientSize(minimumClientSize);
            FormBorderStyle = FormBorderStyle.Sizable;
            MaximizeBox = true;

            flowLayoutPanel.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            lblVideo.MaximumSize = new Size(430, 0);
            lblCourse.MaximumSize = new Size(430, 0);
            lblTotal.MaximumSize = new Size(430, 0);
            Resize += DownloaderForm_Resize;
            UpdateResponsiveLayout();
        }

        private void DownloaderForm_Resize(object sender, EventArgs e)
        {
            UpdateResponsiveLayout();
        }

        private void UpdateResponsiveLayout()
        {
            if (flowLayoutPanel == null)
                return;

            int availableWidth = Math.Max(620, ClientSize.Width - 30);
            flowLayoutPanel.MaximumSize = new Size(availableWidth, 0);
            flowLayoutPanel.Width = availableWidth;
            progressBarCourse.Width = Math.Max(220, availableWidth - 26);
            progressBarTotal.Width = Math.Max(220, availableWidth - 26);
            progressBarVideo.Width = Math.Max(220, availableWidth - 80);
            lblVideo.MaximumSize = new Size(Math.Max(260, availableWidth - 230), 0);
            lblCourse.MaximumSize = new Size(Math.Max(260, availableWidth - 230), 0);
            lblTotal.MaximumSize = new Size(Math.Max(260, availableWidth - 190), 0);
        }

        private async void DownloaderForm_Load(object sender, EventArgs e)
        {
            try
            {
                for (int i = 0; i < _courses.Count; i++)
                {
                    _currentVideoIndex = 1;
                    var course = _courses[i];
                    lblTotal.Text = $"Downloading Course : {course.Title} [{i + 1}/{_courses.Count}]";
                    _cancellationToken.ThrowIfCancellationRequested();
                    await DownloadCourse(course);
                    _cancellationToken.ThrowIfCancellationRequested();
                    progressBarTotal.Value = (i + 1) * 100 / _courses.Count;
                }
                DownloaderStatus = CourseStatus.Finished;
                CloseIfOpen();
            }
            catch (OperationCanceledException) when (_cancellationToken.IsCancellationRequested)
            {
                DownloaderStatus = CourseStatus.Cancelled;
                CloseIfOpen();
            }
            catch (Exception ex)
            {
                Log.Error(ex, "Course download failed");
                DownloaderStatus = CourseStatus.Failed;
                CloseIfOpen();
            }
        }
        private async Task DownloadExerciseFiles(Course course, DirectoryInfo courseDirectory)
        {
            if (_cancellationToken.IsCancellationRequested) return;
            var failedExerciseFiles = new List<string>();
            foreach (var exerciseFile in course.ExerciseFiles)
            {
                try
                {
                    await Retry.Do(async () =>
                    {
                        lblDownloadingVideo.Visible = false;
                        lblVideo.Text = "Downloading exercise file : " + exerciseFile.FileName;
                        string exerciseFilePath = Path.Combine(courseDirectory.FullName, ToSafeFileName(exerciseFile.FileName));
                        await DownloadToFileAsync(new Uri(exerciseFile.DownloadUrl), exerciseFilePath);
                        ExtractExerciseFileArchive(exerciseFilePath);
                    },
                    exceptionMessage: "Failed to download exercise file with name " + exerciseFile.FileName,
                    actionOnError: () => UpdateUI(() => progressBarVideo.Value = 0),
                    retries: 3);
                }
                catch (Exception ex)
                {
                    failedExerciseFiles.Add($"{exerciseFile.FileName}: {ex.Message}");
                    Log.Error(ex, "Skipping exercise file after retry exhaustion: {exerciseFile}", exerciseFile.FileName);
                    UpdateUI(() => lblVideo.Text = "Skipped exercise file after retries: " + exerciseFile.FileName);
                }
            }
            lblDownloadingVideo.Visible = true;
            if (failedExerciseFiles.Count > 0)
            {
                await SaveExerciseFileFailureReport(courseDirectory, failedExerciseFiles);
            }
        }

        private void ExtractExerciseFileArchive(string exerciseFilePath)
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
            UpdateUI(() => lblVideo.Text = "Extracted exercise files to: " + Path.GetFileName(extractionResult.DestinationDirectory));
        }

        private async Task DownloadCourse(Course course)
        {
            try
            {
                _videosCount = course.Chapters.SelectMany(ch => ch.Videos).Count();
                var courseDirectory = _courseRootDirectory.CreateSubdirectory(ToSafeFileName(course.Title));
                if (_toDownloadExerciseFiles && course.ExerciseFiles != null && course.ExerciseFiles.Count > 0)
                {
                    await DownloadExerciseFiles(course, courseDirectory);
                }
                if (!_toDownloadVideos && !_toDownloadSubtitles)
                {
                    lblVideo.Text = "Video downloads skipped";
                    return;
                }

                int i = 1;
                foreach (var chapter in course.Chapters)
                {
                    var chapterDirectory = courseDirectory.CreateSubdirectory($"{i:D2} - {ToSafeFileName(chapter.Title)}");
                    int j = 1;
                    foreach (var video in chapter.Videos)
                    {
                        if (_cancellationToken.IsCancellationRequested) return;
                        await Retry.Do(async () =>
                        {
                            lblVideo.Text = video.Title + " - [Chapter " + i + "]";
                            lblCourse.Text = _currentVideoIndex++ + "/" + _videosCount;

                            string videoName = $"{j:D2} - { ToSafeFileName(video.Title)}.mp4";
                            if (!String.IsNullOrWhiteSpace(video.Transcript) && _toDownloadSubtitles)
                            {
                                string captionName = $"{j:D2} - { ToSafeFileName(video.Title)}.srt";
                                await SaveSubtitles(Path.Combine(chapterDirectory.FullName, ToSafeFileName(captionName)), video.Transcript);
                            }
                            if (_toDownloadVideos)
                            {
                                await DownloadToFileAsync(new Uri(video.DownloadUrl), Path.Combine(chapterDirectory.FullName, videoName));
                            }
                            if (_currentVideoIndex <= _videosCount)
                            {
                                UpdateUI(() => progressBarCourse.Value = _currentVideoIndex * 100 / _videosCount);
                            }
                        },
                        exceptionMessage: "Failed to download video with title " + video.Title,
                        actionOnError: () =>
                        {
                            UpdateUI(() => progressBarVideo.Value = 0);
                            _currentVideoIndex--;
                        },
                        retries: 5);
                        j++;
                    }
                    i++;
                }
            }
            catch (OperationCanceledException)
            {
                throw;
            }
            catch (Exception)
            {
                DownloaderStatus = CourseStatus.Failed;
                CloseIfOpen();
                throw;
            }
        }

        private async Task DownloadToFileAsync(Uri uri, string filePath)
        {
            try
            {
                await _downloader.DownloadFileAsync(uri, filePath, _cancellationToken, DownloadProgressChanged);
            }
            catch (ObjectDisposedException) when (_cancellationToken.IsCancellationRequested)
            {
                throw new OperationCanceledException(_cancellationToken);
            }
        }

        private void DownloadProgressChanged(long downloadedBytes, long totalBytes)
        {
            UpdateUI(() =>
            {
                if (totalBytes <= 0) //sometimes linkedin api doesn't return the file size
                {
                    if (progressBarVideo.Style == ProgressBarStyle.Continuous)
                    {
                        progressBarVideo.Style = ProgressBarStyle.Marquee;
                        lblPercentage.Text = "";
                    }
                }
                else
                {
                    int progressPercentage = Math.Max(0, Math.Min(100, (int)((double)downloadedBytes / (double)totalBytes * 100)));
                    if (progressBarVideo.Style == ProgressBarStyle.Marquee)
                        progressBarVideo.Style = ProgressBarStyle.Continuous;
                    progressBarVideo.Value = progressPercentage;
                    lblPercentage.Text = progressPercentage + "%";
                }
            });
        }

        private void UpdateUI(Action updateAction)
        {
            if (_cancellationToken.IsCancellationRequested || IsDisposed || Disposing || !IsHandleCreated)
                return;

            try
            {
                Invoke(updateAction);
            }
            catch (ObjectDisposedException)
            {
            }
            catch (InvalidOperationException) when (IsDisposed || Disposing || !IsHandleCreated)
            {
            }
        }

        private static string ToSafeFileName(string fileName) => string.Concat(fileName.Split(Path.GetInvalidFileNameChars()));

        private async Task SaveSubtitles(string filePath, string subtitles)
        {
            using (var streamWriter = new StreamWriter(filePath, false))
                await streamWriter.WriteAsync(subtitles);
        }

        private static async Task SaveExerciseFileFailureReport(DirectoryInfo courseDirectory, IEnumerable<string> failures)
        {
            string reportPath = Path.Combine(courseDirectory.FullName, "exercise-file-download-failures.txt");
            using (var streamWriter = new StreamWriter(reportPath, false))
            {
                await streamWriter.WriteLineAsync("Some exercise files could not be downloaded.");
                await streamWriter.WriteLineAsync("If the error says the host cannot be found, the CDN host may be blocked by DNS, VPN, firewall, or network filtering.");
                await streamWriter.WriteLineAsync("Try a different network, disable filtering/VPN temporarily, or download the exercise files from the LinkedIn Learning course Overview tab.");
                await streamWriter.WriteLineAsync();
                foreach (var failure in failures)
                {
                    await streamWriter.WriteLineAsync(failure);
                }
            }
        }

        private void DownloaderForm_FormClosing(object sender, FormClosingEventArgs e)
        {
            if (DownloaderStatus != CourseStatus.Finished && DownloaderStatus != CourseStatus.Failed)
            {
                DownloaderStatus = CourseStatus.Cancelled;
                _cancellationTokenSource.Cancel();
            }
            _downloader.Dispose();
        }

        private void CloseIfOpen()
        {
            if (!IsDisposed && !Disposing)
            {
                Close();
            }
        }
    }
}
