using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Platform.Storage;
using Avalonia.Threading;
using LLCD.CourseContent;
using LLCD.CourseExtractor;
using LLCD.CourseExtractor.YtDlp;
using LLCD.DownloaderConfig;
using Serilog;

namespace LLCD.LinkVault;

public class MainWindow : Window
{
    private static readonly IBrush BackgroundBrush = new SolidColorBrush(Color.Parse("#11131A"));
    private static readonly IBrush SidebarBrush = new SolidColorBrush(Color.Parse("#151824"));
    private static readonly IBrush CardBrush = new SolidColorBrush(Color.Parse("#202432"));
    private static readonly IBrush MutedBrush = new SolidColorBrush(Color.Parse("#AAB3C2"));
    private static readonly IBrush AccentBrush = new SolidColorBrush(Color.Parse("#8F7CFF"));
    private static readonly IBrush SoftBorderBrush = new SolidColorBrush(Color.Parse("#343A4D"));

    private readonly ContentControl _pageHost = new();
    private readonly TextBlock _pageTitle = new();
    private readonly TextBlock _globalStatus = new();
    private readonly List<string> _history = new();
    private CancellationTokenSource? _activeCancellation;
    private TextBlock? _activityLog;
    private string _activityLogText = "";

    private TextBox _linkedInUrls = null!;
    private TextBox _linkedInFolder = null!;
    private TextBox _linkedInToken = null!;
    private TextBox _linkedInDelay = null!;
    private ComboBox _linkedInBrowser = null!;
    private ComboBox _linkedInResolution = null!;
    private CheckBox _linkedInVideos = null!;
    private CheckBox _linkedInExercises = null!;
    private CheckBox _linkedInSubtitles = null!;
    private ProgressBar _linkedInProgress = null!;

    private TextBox _genericUrls = null!;
    private TextBox _genericFolder = null!;
    private TextBox _genericSubtitleLanguages = null!;
    private ComboBox _genericCookies = null!;
    private ComboBox _genericMode = null!;
    private ComboBox _genericAudioFormat = null!;
    private CheckBox _genericPlaylist = null!;
    private CheckBox _genericSubtitles = null!;
    private CheckBox _genericAutoSubtitles = null!;
    private CheckBox _genericInfoJson = null!;
    private CheckBox _genericThumbnail = null!;
    private ProgressBar _genericProgress = null!;
    private TextBlock _genericMetadata = null!;

    public MainWindow()
    {
        Title = "LinkVault";
        Width = 1120;
        Height = 760;
        MinWidth = 760;
        MinHeight = 560;
        Background = BackgroundBrush;
        Content = BuildShell();

        OpenLinkedInPage();
        _ = LoadConfigAsync();
    }

    private Control BuildShell()
    {
        var root = new Grid
        {
            ColumnDefinitions = new ColumnDefinitions("232,*"),
            RowDefinitions = new RowDefinitions("72,*")
        };

        var brand = new StackPanel
        {
            Margin = new Thickness(18, 14, 18, 12),
            Spacing = 2
        };
        brand.Children.Add(new TextBlock
        {
            Text = "LinkVault",
            FontSize = 25,
            FontWeight = FontWeight.SemiBold,
            Foreground = Brushes.White
        });
        brand.Children.Add(new TextBlock
        {
            Text = "Course and video archive",
            FontSize = 12,
            Foreground = MutedBrush
        });
        Grid.SetColumn(brand, 0);
        Grid.SetRow(brand, 0);
        root.Children.Add(brand);

        var sidebar = new Border
        {
            Background = SidebarBrush,
            BorderBrush = SoftBorderBrush,
            BorderThickness = new Thickness(0, 0, 1, 0),
            Child = BuildNavigation()
        };
        Grid.SetColumn(sidebar, 0);
        Grid.SetRow(sidebar, 1);
        root.Children.Add(sidebar);

        var header = new Grid
        {
            ColumnDefinitions = new ColumnDefinitions("*,Auto"),
            Margin = new Thickness(22, 14, 22, 10)
        };
        _pageTitle.FontSize = 24;
        _pageTitle.FontWeight = FontWeight.SemiBold;
        _pageTitle.Foreground = Brushes.White;
        header.Children.Add(_pageTitle);

        _globalStatus.Text = "Ready";
        _globalStatus.Foreground = Brushes.White;
        _globalStatus.Background = new SolidColorBrush(Color.Parse("#263044"));
        _globalStatus.Padding = new Thickness(14, 7);
        _globalStatus.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(_globalStatus, 1);
        header.Children.Add(_globalStatus);
        Grid.SetColumn(header, 1);
        Grid.SetRow(header, 0);
        root.Children.Add(header);

        _pageHost.Margin = new Thickness(22, 0, 22, 22);
        Grid.SetColumn(_pageHost, 1);
        Grid.SetRow(_pageHost, 1);
        root.Children.Add(_pageHost);

        return root;
    }

    private Control BuildNavigation()
    {
        var panel = new StackPanel
        {
            Margin = new Thickness(14),
            Spacing = 8
        };

        panel.Children.Add(NavButton("LinkedIn Courses", OpenLinkedInPage));
        panel.Children.Add(NavButton("Generic Video", OpenGenericVideoPage));
        panel.Children.Add(NavButton("Tools", OpenToolsPage));
        panel.Children.Add(NavButton("History", OpenHistoryPage));
        panel.Children.Add(NavButton("LinkedIn Scraper  Coming soon", () => { }, false));
        panel.Children.Add(NavButton("Settings", OpenSettingsPage));

        return panel;
    }

    private Button NavButton(string text, Action action, bool enabled = true)
    {
        var button = new Button
        {
            Content = text,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Left,
            Padding = new Thickness(14, 11),
            FontSize = 14,
            IsEnabled = enabled,
            Background = enabled ? new SolidColorBrush(Color.Parse("#1D2230")) : new SolidColorBrush(Color.Parse("#171923")),
            Foreground = enabled ? Brushes.White : MutedBrush,
            BorderBrush = SoftBorderBrush
        };
        button.Click += (_, _) => action();
        return button;
    }

    private void OpenLinkedInPage()
    {
        _pageTitle.Text = "LinkedIn Courses";
        var root = PageScroll();
        var layout = new Grid
        {
            ColumnDefinitions = new ColumnDefinitions("2*,*"),
            RowDefinitions = new RowDefinitions("Auto,Auto"),
            ColumnSpacing = 18,
            RowSpacing = 18
        };

        var form = Card("Course Setup");
        form.Children.Add(Labeled("Course URLs", _linkedInUrls = MultiLineTextBox("One course URL per line", 92)));
        form.Children.Add(FolderRow("Download folder", out _linkedInFolder));
        form.Children.Add(TokenRow());
        form.Children.Add(TwoColumnRow(
            Labeled("Video resolution", _linkedInResolution = Combo(new[] { "720 (High)", "540 (Medium)", "360 (Low)" }, 0)),
            Labeled("Browser token source", _linkedInBrowser = Combo(new[] { "Chrome", "Firefox", "Edge" }, 0))));
        _linkedInVideos = Check("Download videos", true);
        _linkedInExercises = Check("Download exercise files", true);
        _linkedInSubtitles = Check("Download subtitles", true);
        var downloadChoices = new StackPanel
        {
            Spacing = 6,
            Children =
            {
                _linkedInVideos,
                _linkedInExercises,
                _linkedInSubtitles
            }
        };
        form.Children.Add(TwoColumnRow(
            Labeled("Delay seconds", _linkedInDelay = TextBox("0")),
            downloadChoices));
        form.Children.Add(ActionRow(
            Button("Import Token", ImportLinkedInTokenAsync),
            Button("Fetch And Download", StartLinkedInDownloadAsync),
            Button("Cancel", CancelActiveWork)));
        AddCard(layout, form, 0);

        var status = Card("Activity");
        _linkedInProgress = new ProgressBar { Minimum = 0, Maximum = 100, Height = 18 };
        status.Children.Add(StatusBlock("The LinkedIn downloader keeps videos, subtitles, and exercise files as separate choices."));
        status.Children.Add(_linkedInProgress);
        status.Children.Add(LogPanel());
        AddCard(layout, status, 1);

        var notes = Card("Future LinkedIn Scraper");
        notes.Children.Add(StatusBlock("Coming soon. This area is intentionally disabled until the scraper behavior is designed and implemented."));
        AddCard(layout, notes, 0, 1, 2);

        root.Content = layout;
        _pageHost.Content = root;
    }

    private Control TokenRow()
    {
        var grid = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto"), ColumnSpacing = 10 };
        _linkedInToken = TextBox("");
        grid.Children.Add(_linkedInToken);
        var paste = Button("Clear", () => _linkedInToken.Text = "");
        Grid.SetColumn(paste, 1);
        grid.Children.Add(paste);
        return Labeled("Token cookie", grid);
    }

    private void OpenGenericVideoPage()
    {
        _pageTitle.Text = "Generic Video";
        var root = PageScroll();
        var layout = new Grid
        {
            ColumnDefinitions = new ColumnDefinitions("2*,*"),
            ColumnSpacing = 18
        };

        var form = Card("Video Job");
        form.Children.Add(Labeled("Video URLs", _genericUrls = MultiLineTextBox("One URL per line", 88)));
        form.Children.Add(FolderRow("Download folder", out _genericFolder));
        form.Children.Add(TwoColumnRow(
            Labeled("Browser cookies", _genericCookies = Combo(new[] { "None", "Chrome", "Firefox", "Edge" }, 0)),
            Labeled("Mode", _genericMode = Combo(new[] { "Video", "Audio" }, 0))));
        form.Children.Add(TwoColumnRow(
            Labeled("Audio format", _genericAudioFormat = Combo(new[] { "mp3", "m4a", "flac", "wav" }, 0)),
            Labeled("Subtitle languages", _genericSubtitleLanguages = TextBox("en"))));
        _genericPlaylist = Check("Auto-detect playlist", true);
        _genericSubtitles = Check("Download subtitles", false);
        _genericAutoSubtitles = Check("Use auto captions", false);
        _genericInfoJson = Check("Save info JSON", false);
        _genericThumbnail = Check("Save thumbnail", false);
        form.Children.Add(new StackPanel
        {
            Spacing = 6,
            Children =
            {
                _genericPlaylist,
                _genericSubtitles,
                _genericAutoSubtitles,
                _genericInfoJson,
                _genericThumbnail
            }
        });
        form.Children.Add(ActionRow(
            Button("Fetch Metadata", FetchGenericMetadataAsync),
            Button("Download", StartGenericDownloadAsync),
            Button("Cancel", CancelActiveWork)));
        AddCard(layout, form, 0);

        var status = Card("Queue And Logs");
        _genericProgress = new ProgressBar { Minimum = 0, Maximum = 100, Height = 18 };
        _genericMetadata = StatusBlock("No metadata loaded.");
        status.Children.Add(_genericMetadata);
        status.Children.Add(_genericProgress);
        status.Children.Add(LogPanel());
        AddCard(layout, status, 1);

        root.Content = layout;
        _pageHost.Content = root;
    }

    private void OpenToolsPage()
    {
        _pageTitle.Text = "Tools";
        var root = PageScroll();
        var card = Card("Dependencies");
        card.Children.Add(StatusBlock("LinkVault checks for yt-dlp and FFmpeg in PATH and in the app tools folder."));
        card.Children.Add(ActionRow(Button("Check Tools", CheckToolsAsync), Button("Install Tools", InstallToolsAsync), Button("Open App Folder", () => OpenFolder(AppContext.BaseDirectory))));
        card.Children.Add(LogPanel());
        root.Content = CardFrame(card);
        _pageHost.Content = root;
    }

    private void OpenHistoryPage()
    {
        _pageTitle.Text = "History";
        var root = PageScroll();
        var card = Card("Recent Activity");
        card.Children.Add(StatusBlock(_history.Count == 0 ? "No completed jobs in this session yet." : String.Join(Environment.NewLine, _history.TakeLast(20))));
        root.Content = CardFrame(card);
        _pageHost.Content = root;
    }

    private void OpenSettingsPage()
    {
        _pageTitle.Text = "Settings";
        var root = PageScroll();
        var card = Card("App Settings");
        card.Children.Add(StatusBlock("Config path: " + Path.GetFullPath("./Config.json")));
        card.Children.Add(StatusBlock("Theme: Dark"));
        card.Children.Add(StatusBlock("Default shell: LinkVault Avalonia UI"));
        card.Children.Add(ActionRow(Button("Open Config Folder", () => OpenFolder(Directory.GetCurrentDirectory()))));
        root.Content = CardFrame(card);
        _pageHost.Content = root;
    }

    private async Task ImportLinkedInTokenAsync()
    {
        await RunBusyAsync("Checking browser token...", async token =>
        {
            var browser = (Browser)Math.Max(0, _linkedInBrowser.SelectedIndex);
            var browsers = new[] { browser }.Concat(Enum.GetValues(typeof(Browser)).Cast<Browser>()).Distinct();
            foreach (var candidate in browsers)
            {
                var value = await Extractor.ExtractValidToken(candidate);
                token.ThrowIfCancellationRequested();
                if (!String.IsNullOrWhiteSpace(value))
                {
                    _linkedInBrowser.SelectedIndex = (int)candidate;
                    _linkedInToken.Text = value;
                    LogLine("Imported LinkedIn token from " + candidate + ".");
                    return;
                }
            }
            LogLine("No valid LinkedIn token found in supported browsers.");
        });
    }

    private async Task StartLinkedInDownloadAsync()
    {
        await RunBusyAsync("Downloading LinkedIn courses...", async token =>
        {
            await SaveConfigAsync();
            var urls = SplitLines(_linkedInUrls.Text).ToList();
            if (urls.Count == 0)
                throw new InvalidOperationException("Add at least one LinkedIn course URL.");
            if (String.IsNullOrWhiteSpace(_linkedInFolder.Text))
                throw new InvalidOperationException("Choose a download folder.");
            if (String.IsNullOrWhiteSpace(_linkedInToken.Text))
                throw new InvalidOperationException("Import or paste a LinkedIn token.");

            var root = new DirectoryInfo(_linkedInFolder.Text.Trim());
            root.Create();
            var quality = (Quality)Math.Max(0, _linkedInResolution.SelectedIndex);
            bool includeVideoDetails = IsChecked(_linkedInVideos) || IsChecked(_linkedInSubtitles);
            int delay = Int32.TryParse(_linkedInDelay.Text, out var parsedDelay) ? Math.Max(0, parsedDelay) : 0;

            int courseIndex = 0;
            foreach (var url in urls)
            {
                courseIndex++;
                token.ThrowIfCancellationRequested();
                SetProgress(_linkedInProgress, 0);
                LogLine($"Extracting {courseIndex}/{urls.Count}: {url}");
                var extractor = new Extractor(url, quality, _linkedInToken.Text.Trim(), delay);
                if (!extractor.HasValidUrl())
                    throw new InvalidOperationException("Invalid LinkedIn Learning URL: " + url);
                if (!await extractor.HasValidToken())
                    throw new InvalidOperationException("LinkedIn token is invalid or expired.");

                var progress = new Progress<float>(value => SetProgress(_linkedInProgress, value * 45));
                var course = await extractor.GetCourse(progress, includeVideoDetails);
                await DownloadCourseAsync(course, root, IsChecked(_linkedInVideos), IsChecked(_linkedInExercises), IsChecked(_linkedInSubtitles), percent => SetProgress(_linkedInProgress, 45 + percent * 55), token);
                _history.Add("LinkedIn: " + course.Title);
                LogLine("Finished course: " + course.Title);
            }
        });
    }

    private async Task FetchGenericMetadataAsync()
    {
        await RunBusyAsync("Fetching metadata...", async token =>
        {
            var url = SplitLines(_genericUrls.Text).FirstOrDefault();
            if (String.IsNullOrWhiteSpace(url))
                throw new InvalidOperationException("Add a video URL first.");

            var service = CreateYtDlpService();
            var info = await service.GetInfo(url, GetCookiesSource(), token);
            _genericMetadata.Text = $"{info.Title ?? "Untitled"}\nDuration: {FormatDuration(info.Duration)}\nUploader: {info.Uploader ?? "Unknown"}";
            LogLine("Metadata loaded: " + (info.Title ?? url));
        });
    }

    private async Task StartGenericDownloadAsync()
    {
        await RunBusyAsync("Downloading generic videos...", async token =>
        {
            await SaveConfigAsync();
            var urls = SplitLines(_genericUrls.Text).ToList();
            if (urls.Count == 0)
                throw new InvalidOperationException("Add at least one video URL.");
            if (String.IsNullOrWhiteSpace(_genericFolder.Text))
                throw new InvalidOperationException("Choose a download folder.");

            Directory.CreateDirectory(_genericFolder.Text.Trim());
            var service = CreateYtDlpService();
            var runner = new YtDlpJobRunner(service);
            int index = 0;
            foreach (var url in urls)
            {
                token.ThrowIfCancellationRequested();
                index++;
                SetProgress(_genericProgress, 0);
                var job = new YtDlpJob
                {
                    Url = url,
                    Options = BuildYtDlpOptions(url)
                };
                LogLine($"Queued generic video {index}/{urls.Count}: {url}");
                var result = await runner.Download(job, token, progress =>
                {
                    if (progress.Percent.HasValue)
                        SetProgress(_genericProgress, progress.Percent.Value);
                    if (!String.IsNullOrWhiteSpace(progress.RawLine))
                        LogLine(progress.RawLine);
                });
                if (!result.Success)
                    throw new InvalidOperationException(result.Error ?? "yt-dlp download failed.");
                _history.Add("Generic video: " + (result.FileName ?? url));
                LogLine("Finished: " + (result.FileName ?? url));
            }
        });
    }

    private async Task CheckToolsAsync()
    {
        await RunBusyAsync("Checking tools...", _ =>
        {
            var status = YtDlpDependencyChecker.Check(AppContext.BaseDirectory);
            LogLine("yt-dlp: " + (status.HasYtDlp ? status.YtDlpPath : "Missing"));
            LogLine("ffmpeg: " + (status.HasFfmpeg ? status.FfmpegPath : "Missing"));
            LogLine("ffprobe: " + (status.HasFfprobe ? status.FfprobePath : "Missing"));
            return Task.CompletedTask;
        });
    }

    private async Task InstallToolsAsync()
    {
        await RunBusyAsync("Installing tools...", async token =>
        {
            var installer = new YtDlpDependencyInstaller();
            await installer.InstallAllAsync(AppContext.BaseDirectory, token, progress =>
            {
                if (progress.Percent.HasValue)
                    SetStatus($"{progress.Message}: {progress.Percent.Value:0.0}%");
                LogLine(progress.Message);
            });
            LogLine("Tool install complete.");
        });
    }

    private YtDlpService CreateYtDlpService()
    {
        var status = YtDlpDependencyChecker.Check(AppContext.BaseDirectory);
        return new YtDlpService(status.HasYtDlp ? status.YtDlpPath : "yt-dlp");
    }

    private YtDlpDownloadOptions BuildYtDlpOptions(string url)
    {
        var status = YtDlpDependencyChecker.Check(AppContext.BaseDirectory);
        return new YtDlpDownloadOptions
        {
            Url = url,
            OutputTemplate = Path.Combine(_genericFolder.Text?.Trim() ?? "", "%(title)s.%(ext)s"),
            FfmpegLocation = status.HasFfmpeg ? Path.GetDirectoryName(status.FfmpegPath) : null,
            CookiesSource = GetCookiesSource(),
            FormatChoice = _genericMode.SelectedIndex == 1 ? YtDlpFormatChoice.Audio : YtDlpFormatChoice.Video,
            AudioFormat = GetAudioFormat(),
            NoPlaylist = !IsChecked(_genericPlaylist),
            WriteSubtitles = IsChecked(_genericSubtitles),
            WriteAutomaticSubtitles = IsChecked(_genericAutoSubtitles),
            SubtitleLanguages = String.IsNullOrWhiteSpace(_genericSubtitleLanguages.Text) ? "en" : _genericSubtitleLanguages.Text.Trim(),
            WriteInfoJson = IsChecked(_genericInfoJson),
            WriteThumbnail = IsChecked(_genericThumbnail)
        };
    }

    private YtDlpBrowserCookiesSource GetCookiesSource()
    {
        return _genericCookies.SelectedIndex switch
        {
            1 => YtDlpBrowserCookiesSource.Chrome,
            2 => YtDlpBrowserCookiesSource.Firefox,
            3 => YtDlpBrowserCookiesSource.Edge,
            _ => YtDlpBrowserCookiesSource.None
        };
    }

    private YtDlpAudioFormat GetAudioFormat()
    {
        return _genericAudioFormat.SelectedIndex switch
        {
            1 => YtDlpAudioFormat.M4a,
            2 => YtDlpAudioFormat.Flac,
            3 => YtDlpAudioFormat.Wav,
            _ => YtDlpAudioFormat.Mp3
        };
    }

    private async Task DownloadCourseAsync(Course course, DirectoryInfo root, bool videos, bool exercises, bool subtitles, Action<double> progress, CancellationToken token)
    {
        var courseDirectory = root.CreateSubdirectory(ToSafeFileName(course.Title));
        int totalSteps = (exercises ? course.ExerciseFiles?.Count ?? 0 : 0) + (videos || subtitles ? course.Chapters.Sum(chapter => chapter.Videos.Count) : 0);
        totalSteps = Math.Max(1, totalSteps);
        int completed = 0;

        if (exercises && course.ExerciseFiles != null)
        {
            foreach (var exerciseFile in course.ExerciseFiles)
            {
                token.ThrowIfCancellationRequested();
                var path = Path.Combine(courseDirectory.FullName, ToSafeFileName(exerciseFile.FileName));
                LogLine("Downloading exercise file: " + exerciseFile.FileName);
                await DownloadFileAsync(new Uri(exerciseFile.DownloadUrl), path, token);
                var extract = ExerciseArchiveExtractor.ExtractZipAndDeleteArchive(path);
                if (extract.Attempted && !extract.Succeeded)
                    LogLine("Exercise zip kept because extraction failed: " + extract.Message);
                completed++;
                progress((double)completed / totalSteps);
            }
        }

        if (!videos && !subtitles)
            return;

        for (int i = 0; i < course.Chapters.Count; i++)
        {
            var chapter = course.Chapters[i];
            var chapterDirectory = courseDirectory.CreateSubdirectory($"{i + 1:D2} - {ToSafeFileName(chapter.Title)}");
            for (int j = 0; j < chapter.Videos.Count; j++)
            {
                token.ThrowIfCancellationRequested();
                var video = chapter.Videos[j];
                var baseName = $"{j + 1:D2} - {ToSafeFileName(video.Title)}";
                if (subtitles && !String.IsNullOrWhiteSpace(video.Transcript))
                    await File.WriteAllTextAsync(Path.Combine(chapterDirectory.FullName, baseName + ".srt"), video.Transcript, token);
                if (videos)
                {
                    LogLine("Downloading video: " + video.Title);
                    await DownloadFileAsync(new Uri(video.DownloadUrl), Path.Combine(chapterDirectory.FullName, baseName + ".mp4"), token);
                }
                completed++;
                progress((double)completed / totalSteps);
            }
        }
    }

    private static async Task DownloadFileAsync(Uri uri, string filePath, CancellationToken token)
    {
        string tempPath = filePath + ".download";
        Directory.CreateDirectory(Path.GetDirectoryName(filePath)!);
        try
        {
            using var client = new HttpClient { Timeout = TimeSpan.FromMinutes(30) };
            using var response = await client.GetAsync(uri, HttpCompletionOption.ResponseHeadersRead, token);
            response.EnsureSuccessStatusCode();
            await using var source = await response.Content.ReadAsStreamAsync(token);
            await using var target = File.Create(tempPath);
            await source.CopyToAsync(target, token);
            target.Close();
            if (File.Exists(filePath))
                File.Delete(filePath);
            File.Move(tempPath, filePath);
        }
        finally
        {
            TryDelete(tempPath);
        }
    }

    private async Task LoadConfigAsync()
    {
        if (!File.Exists("./Config.json"))
            return;

        try
        {
            var config = await Config.Fill();
            if (_linkedInFolder != null && config.CourseDirectory != null)
                _linkedInFolder.Text = config.CourseDirectory.FullName;
            if (_genericFolder != null && config.YtDlpDownloadDirectory != null)
                _genericFolder.Text = config.YtDlpDownloadDirectory.FullName;
            if (_linkedInToken != null)
                _linkedInToken.Text = config.AuthenticationToken ?? "";
            if (_linkedInResolution != null)
                _linkedInResolution.SelectedIndex = Math.Clamp((int)config.Quality, 0, 2);
            if (_linkedInVideos != null)
                _linkedInVideos.IsChecked = config.DownloadVideos;
            if (_linkedInExercises != null)
                _linkedInExercises.IsChecked = config.DownloadExerciseFiles;
            if (_linkedInSubtitles != null)
                _linkedInSubtitles.IsChecked = config.DownloadSubtitles;
            if (_genericCookies != null)
                _genericCookies.SelectedIndex = Math.Clamp(config.YtDlpCookiesSourceIndex, 0, 3);
            if (_genericMode != null)
                _genericMode.SelectedIndex = Math.Clamp(config.YtDlpFormatTypeIndex, 0, 1);
        }
        catch (Exception ex)
        {
            Log.Error(ex, "Failed to load config");
            LogLine("Config could not be loaded.");
        }
    }

    private async Task SaveConfigAsync()
    {
        var config = File.Exists("./Config.json") ? await Config.Fill() : new Config();
        config.AuthenticationToken = _linkedInToken?.Text ?? "";
        config.Quality = (Quality)Math.Max(0, _linkedInResolution?.SelectedIndex ?? 0);
        config.DownloadVideos = IsChecked(_linkedInVideos);
        config.DownloadExerciseFiles = IsChecked(_linkedInExercises);
        config.DownloadSubtitles = IsChecked(_linkedInSubtitles);

        if (!String.IsNullOrWhiteSpace(_linkedInFolder?.Text))
            config.CourseDirectory = new DirectoryInfo(_linkedInFolder.Text.Trim());
        if (!String.IsNullOrWhiteSpace(_genericFolder?.Text))
            config.YtDlpDownloadDirectory = new DirectoryInfo(_genericFolder.Text.Trim());

        config.YtDlpCookiesSourceIndex = _genericCookies?.SelectedIndex ?? 0;
        config.YtDlpFormatTypeIndex = _genericMode?.SelectedIndex ?? 0;
        config.YtDlpAudioFormatIndex = _genericAudioFormat?.SelectedIndex ?? 0;
        config.YtDlpDownloadSubtitles = IsChecked(_genericSubtitles);
        config.YtDlpDownloadAutoSubtitles = IsChecked(_genericAutoSubtitles);
        config.YtDlpWriteInfoJson = IsChecked(_genericInfoJson);
        config.YtDlpWriteThumbnail = IsChecked(_genericThumbnail);
        config.YtDlpAutoDetectPlaylist = IsChecked(_genericPlaylist);
        config.YtDlpSubtitleLanguages = _genericSubtitleLanguages?.Text ?? "en";
        await config.Save();
    }

    private async Task RunBusyAsync(string status, Func<CancellationToken, Task> work)
    {
        if (_activeCancellation != null)
            return;

        _activeCancellation = new CancellationTokenSource();
        SetStatus(status);
        try
        {
            await work(_activeCancellation.Token);
            SetStatus("Ready");
        }
        catch (OperationCanceledException)
        {
            SetStatus("Cancelled");
            LogLine("Cancelled.");
        }
        catch (Exception ex)
        {
            SetStatus("Error");
            Log.Error(ex, "LinkVault operation failed");
            LogLine("Error: " + ex.Message);
        }
        finally
        {
            _activeCancellation.Dispose();
            _activeCancellation = null;
        }
    }

    private void CancelActiveWork()
    {
        _activeCancellation?.Cancel();
    }

    private ScrollViewer PageScroll()
    {
        return new ScrollViewer
        {
            HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto
        };
    }

    private StackPanel Card(string title)
    {
        var panel = new StackPanel { Spacing = 14 };
        panel.Children.Add(new TextBlock
        {
            Text = title,
            FontSize = 17,
            FontWeight = FontWeight.SemiBold,
            Foreground = Brushes.White
        });

        return panel;
    }

    private Border CardFrame(StackPanel panel)
    {
        return new Border
        {
            Background = CardBrush,
            BorderBrush = SoftBorderBrush,
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(18),
            Child = panel
        };
    }

    private void AddCard(Grid grid, StackPanel panel, int column, int row = 0, int columnSpan = 1)
    {
        var frame = CardFrame(panel);
        Grid.SetColumn(frame, column);
        Grid.SetRow(frame, row);
        if (columnSpan > 1)
            Grid.SetColumnSpan(frame, columnSpan);
        grid.Children.Add(frame);
    }

    private Control Labeled(string label, Control control)
    {
        return new StackPanel
        {
            Spacing = 6,
            Children =
            {
                new TextBlock { Text = label, Foreground = MutedBrush, FontSize = 12 },
                control
            }
        };
    }

    private Control FolderRow(string label, out TextBox textBox)
    {
        var grid = new Grid { ColumnDefinitions = new ColumnDefinitions("*,Auto"), ColumnSpacing = 10 };
        textBox = TextBox("");
        grid.Children.Add(textBox);
        var captured = textBox;
        var browse = Button("Browse", async () => await BrowseFolderAsync(captured));
        Grid.SetColumn(browse, 1);
        grid.Children.Add(browse);
        return Labeled(label, grid);
    }

    private async Task BrowseFolderAsync(TextBox target)
    {
        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions
        {
            Title = "Choose download folder",
            AllowMultiple = false
        });
        var folder = folders.FirstOrDefault();
        if (folder?.Path.LocalPath != null)
            target.Text = folder.Path.LocalPath;
    }

    private Control TwoColumnRow(Control left, Control right)
    {
        var grid = new Grid { ColumnDefinitions = new ColumnDefinitions("*,*"), ColumnSpacing = 14 };
        grid.Children.Add(left);
        Grid.SetColumn(right, 1);
        grid.Children.Add(right);
        return grid;
    }

    private Control ActionRow(params Button[] buttons)
    {
        var panel = new WrapPanel
        {
            Orientation = Orientation.Horizontal,
            HorizontalAlignment = HorizontalAlignment.Left
        };
        foreach (var button in buttons)
        {
            button.Margin = new Thickness(0, 0, 10, 10);
            panel.Children.Add(button);
        }
        return panel;
    }

    private TextBox TextBox(string text)
    {
        return new TextBox
        {
            Text = text,
            MinHeight = 36,
            FontSize = 14,
            Background = new SolidColorBrush(Color.Parse("#151924")),
            BorderBrush = SoftBorderBrush,
            Foreground = Brushes.White
        };
    }

    private TextBox MultiLineTextBox(string watermark, double height)
    {
        return new TextBox
        {
            PlaceholderText = watermark,
            AcceptsReturn = true,
            TextWrapping = TextWrapping.NoWrap,
            Height = height,
            FontSize = 14,
            Background = new SolidColorBrush(Color.Parse("#151924")),
            BorderBrush = SoftBorderBrush,
            Foreground = Brushes.White
        };
    }

    private ComboBox Combo(IEnumerable<string> items, int index)
    {
        return new ComboBox
        {
            ItemsSource = items.ToList(),
            SelectedIndex = index,
            MinHeight = 36,
            Background = new SolidColorBrush(Color.Parse("#151924")),
            BorderBrush = SoftBorderBrush,
            Foreground = Brushes.White
        };
    }

    private CheckBox Check(string text, bool isChecked)
    {
        return new CheckBox
        {
            Content = text,
            IsChecked = isChecked,
            Foreground = Brushes.White,
            FontSize = 14
        };
    }

    private Button Button(string text, Func<Task> action)
    {
        var button = Button(text, () => { });
        button.Click += async (_, _) =>
        {
            try
            {
                await action();
            }
            catch (Exception ex)
            {
                SetStatus("Error");
                Log.Error(ex, "LinkVault button action failed");
                LogLine("Error: " + ex.Message);
            }
        };
        return button;
    }

    private Button Button(string text, Action action)
    {
        var button = new Button
        {
            Content = text,
            Padding = new Thickness(16, 10),
            MinWidth = 112,
            Background = AccentBrush,
            Foreground = Brushes.White,
            BorderBrush = AccentBrush
        };
        button.Click += (_, _) =>
        {
            try
            {
                action();
            }
            catch (Exception ex)
            {
                SetStatus("Error");
                Log.Error(ex, "LinkVault button action failed");
                LogLine("Error: " + ex.Message);
            }
        };
        return button;
    }

    private TextBlock StatusBlock(string text)
    {
        return new TextBlock
        {
            Text = text,
            TextWrapping = TextWrapping.Wrap,
            Foreground = MutedBrush,
            FontSize = 13,
            LineHeight = 19
        };
    }

    private Control LogPanel()
    {
        var log = new TextBlock
        {
            Text = _activityLogText,
            TextWrapping = TextWrapping.Wrap,
            Foreground = MutedBrush,
            FontSize = 12
        };
        _activityLog = log;
        return new Border
        {
            Background = new SolidColorBrush(Color.Parse("#151924")),
            BorderBrush = SoftBorderBrush,
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(6),
            Padding = new Thickness(12),
            MinHeight = 220,
            Child = new ScrollViewer
            {
                VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
                Content = log
            }
        };
    }

    private void SetStatus(string text)
    {
        Dispatcher.UIThread.Post(() => _globalStatus.Text = text);
    }

    private void SetProgress(ProgressBar? progressBar, double value)
    {
        if (progressBar == null)
            return;

        Dispatcher.UIThread.Post(() => progressBar.Value = Math.Clamp(value, 0, 100));
    }

    private void LogLine(string line)
    {
        if (String.IsNullOrWhiteSpace(line))
            return;
        Dispatcher.UIThread.Post(() =>
        {
            _activityLogText = (_activityLogText + $"[{DateTime.Now:HH:mm:ss}] {line}" + Environment.NewLine).TrimStart();
            if (_activityLog != null)
                _activityLog.Text = _activityLogText;
        });
    }

    private static bool IsChecked(CheckBox? checkBox)
    {
        return checkBox?.IsChecked == true;
    }

    private static IEnumerable<string> SplitLines(string? text)
    {
        return (text ?? "")
            .Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries)
            .Select(item => item.Trim())
            .Where(item => !String.IsNullOrWhiteSpace(item));
    }

    private static string ToSafeFileName(string fileName)
    {
        return string.Concat(fileName.Split(Path.GetInvalidFileNameChars()));
    }

    private static string FormatDuration(double? seconds)
    {
        if (!seconds.HasValue || seconds.Value <= 0)
            return "Unknown";

        return TimeSpan.FromSeconds(seconds.Value).ToString(@"hh\:mm\:ss");
    }

    private static void OpenFolder(string folder)
    {
        Directory.CreateDirectory(folder);
        Process.Start(new ProcessStartInfo { FileName = folder, UseShellExecute = true });
    }

    private static void TryDelete(string path)
    {
        try
        {
            if (File.Exists(path))
                File.Delete(path);
        }
        catch
        {
        }
    }
}
