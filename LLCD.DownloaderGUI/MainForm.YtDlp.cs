using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using System.Windows.Forms;
using LLCD.CourseExtractor.YtDlp;
using LLCD.DownloaderConfig;
using Serilog;

namespace LLCD.DownloaderGUI
{
    public partial class MainForm
    {
        private TabControl tabControlMain;
        private TextBox txtYtDlpUrl;
        private TextBox txtYtDlpDirectory;
        private TextBox txtYtDlpSubtitleLanguages;
        private TextBox txtYtDlpLog;
        private ComboBox cmboxYtDlpCookies;
        private ComboBox cmboxYtDlpFormatType;
        private ComboBox cmboxYtDlpAudioFormat;
        private ComboBox cmboxYtDlpVideoFormat;
        private CheckBox checkBoxYtDlpSubtitles;
        private CheckBox checkBoxYtDlpAutoSubtitles;
        private CheckBox checkBoxYtDlpInfoJson;
        private CheckBox checkBoxYtDlpThumbnail;
        private CheckBox checkBoxYtDlpPlaylist;
        private Label lblYtDlpStatus;
        private Label lblYtDlpInfo;
        private ListBox listBoxYtDlpQueue;
        private ProgressBar progressBarYtDlp;
        private Button btnYtDlpCheckDependencies;
        private Button btnYtDlpInstallTools;
        private Button btnYtDlpFetchInfo;
        private Button btnYtDlpDownload;
        private Button btnYtDlpCancel;
        private Button btnYtDlpRetryFailed;
        private Button btnYtDlpRemoveSelected;
        private Button btnYtDlpOpenFolder;

        private YtDlpDependencyStatus _ytDlpDependencies;
        private YtDlpInfo _ytDlpInfo;
        private List<YtDlpJob> _ytDlpQueue = new List<YtDlpJob>();
        private List<YtDlpFormat> _ytDlpFormats = new List<YtDlpFormat>();
        private CancellationTokenSource _ytDlpCancellationTokenSource;
        private bool _isYtDlpBusy;
        private string _ytDlpQueueSourceKey;

        private void InitializeYtDlpTab()
        {
            ConfigureMainWindowForTabs();

            var linkedInTab = new TabPage("LinkedIn Learning")
            {
                BackColor = Color.FromArgb(18, 18, 18),
                ForeColor = Color.White,
                AutoScroll = true,
                AutoScrollMinSize = new Size(710, 710)
            };
            var ytDlpTab = new TabPage("Generic video")
            {
                BackColor = Color.FromArgb(18, 18, 18),
                ForeColor = Color.White,
                AutoScroll = true,
                AutoScrollMinSize = new Size(700, 810)
            };

            Controls.Remove(panelBody);
            panelBody.Location = new Point(8, 8);
            linkedInTab.Controls.Add(panelBody);
            ConfigureLinkedInTabLayout(linkedInTab);

            tabControlMain = new TabControl
            {
                Dock = DockStyle.Fill,
                Font = new Font("Segoe UI", 10F),
                SelectedIndex = 0
            };
            tabControlMain.Controls.Add(linkedInTab);
            tabControlMain.Controls.Add(ytDlpTab);
            Controls.Add(tabControlMain);

            BuildYtDlpTab(ytDlpTab);
        }

        private void BuildYtDlpTab(TabPage tabPage)
        {
            var panel = new Panel
            {
                Anchor = AnchorStyles.Top | AnchorStyles.Bottom | AnchorStyles.Left | AnchorStyles.Right,
                AutoScroll = true,
                AutoScrollMinSize = new Size(671, 790),
                BackColor = Color.FromArgb(29, 29, 29),
                Location = new Point(13, 13),
                MinimumSize = new Size(671, 790),
                Size = new Size(Math.Max(671, tabPage.ClientSize.Width - 26), Math.Max(790, tabPage.ClientSize.Height - 26))
            };
            panel.Resize += panelYtDlp_Resize;
            tabPage.Controls.Add(panel);

            panel.Controls.Add(CreateYtDlpLabel("Video URLs:", 16, 18, 170));
            txtYtDlpUrl = CreateYtDlpTextBox(187, 16, 463);
            txtYtDlpUrl.AcceptsReturn = true;
            txtYtDlpUrl.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            txtYtDlpUrl.Height = 78;
            txtYtDlpUrl.Multiline = true;
            txtYtDlpUrl.ScrollBars = ScrollBars.Vertical;
            txtYtDlpUrl.WordWrap = false;
            panel.Controls.Add(txtYtDlpUrl);

            panel.Controls.Add(CreateYtDlpLabel("Download folder:", 16, 61, 170));
            txtYtDlpDirectory = CreateYtDlpTextBox(187, 59, 422);
            txtYtDlpDirectory.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            panel.Controls.Add(txtYtDlpDirectory);
            var btnYtDlpBrowse = CreateYtDlpButton("...", 615, 58, 35, 33);
            btnYtDlpBrowse.Anchor = AnchorStyles.Top | AnchorStyles.Right;
            btnYtDlpBrowse.Click += btnYtDlpBrowse_Click;
            panel.Controls.Add(btnYtDlpBrowse);

            panel.Controls.Add(CreateYtDlpLabel("Browser cookies:", 16, 104, 170));
            cmboxYtDlpCookies = CreateYtDlpComboBox(187, 102, 223);
            cmboxYtDlpCookies.Items.AddRange(new object[] { "None", "Chrome", "Firefox", "Edge" });
            cmboxYtDlpCookies.SelectedIndex = 0;
            panel.Controls.Add(cmboxYtDlpCookies);

            panel.Controls.Add(CreateYtDlpLabel("Mode:", 430, 104, 70));
            cmboxYtDlpFormatType = CreateYtDlpComboBox(500, 102, 150);
            cmboxYtDlpFormatType.Anchor = AnchorStyles.Top | AnchorStyles.Right;
            cmboxYtDlpFormatType.Items.AddRange(new object[] { "Video", "Audio" });
            cmboxYtDlpFormatType.SelectedIndex = 0;
            cmboxYtDlpFormatType.SelectedIndexChanged += cmboxYtDlpFormatType_SelectedIndexChanged;
            panel.Controls.Add(cmboxYtDlpFormatType);

            panel.Controls.Add(CreateYtDlpLabel("Video format:", 16, 147, 170));
            cmboxYtDlpVideoFormat = CreateYtDlpComboBox(187, 145, 223);
            cmboxYtDlpVideoFormat.Items.Add("Best available");
            cmboxYtDlpVideoFormat.SelectedIndex = 0;
            panel.Controls.Add(cmboxYtDlpVideoFormat);

            panel.Controls.Add(CreateYtDlpLabel("Audio format:", 430, 147, 120));
            cmboxYtDlpAudioFormat = CreateYtDlpComboBox(550, 145, 100);
            cmboxYtDlpAudioFormat.Anchor = AnchorStyles.Top | AnchorStyles.Right;
            cmboxYtDlpAudioFormat.Items.AddRange(new object[] { "mp3", "m4a", "flac", "wav" });
            cmboxYtDlpAudioFormat.SelectedIndex = 0;
            panel.Controls.Add(cmboxYtDlpAudioFormat);

            checkBoxYtDlpSubtitles = CreateYtDlpCheckBox("Download subtitles", 16, 194, 230);
            panel.Controls.Add(checkBoxYtDlpSubtitles);
            checkBoxYtDlpAutoSubtitles = CreateYtDlpCheckBox("Use auto captions", 254, 194, 220);
            panel.Controls.Add(checkBoxYtDlpAutoSubtitles);

            checkBoxYtDlpInfoJson = CreateYtDlpCheckBox("Save info JSON", 16, 232, 210);
            panel.Controls.Add(checkBoxYtDlpInfoJson);
            checkBoxYtDlpThumbnail = CreateYtDlpCheckBox("Save thumbnail", 254, 232, 210);
            panel.Controls.Add(checkBoxYtDlpThumbnail);
            checkBoxYtDlpPlaylist = CreateYtDlpCheckBox("Auto-detect playlist", 455, 232, 200);
            panel.Controls.Add(checkBoxYtDlpPlaylist);

            panel.Controls.Add(CreateYtDlpLabel("Subtitle langs:", 16, 270, 170));
            txtYtDlpSubtitleLanguages = CreateYtDlpTextBox(187, 268, 223);
            txtYtDlpSubtitleLanguages.Text = "en";
            panel.Controls.Add(txtYtDlpSubtitleLanguages);

            btnYtDlpCheckDependencies = CreateYtDlpButton("CHECK", 16, 320, 122, 44);
            btnYtDlpCheckDependencies.Click += btnYtDlpCheckDependencies_Click;
            panel.Controls.Add(btnYtDlpCheckDependencies);

            btnYtDlpInstallTools = CreateYtDlpButton("TOOLS", 144, 320, 122, 44);
            btnYtDlpInstallTools.Click += btnYtDlpInstallTools_Click;
            panel.Controls.Add(btnYtDlpInstallTools);

            btnYtDlpFetchInfo = CreateYtDlpButton("FETCH", 272, 320, 122, 44);
            btnYtDlpFetchInfo.Click += btnYtDlpFetchInfo_Click;
            panel.Controls.Add(btnYtDlpFetchInfo);

            btnYtDlpDownload = CreateYtDlpButton("DOWNLOAD", 400, 320, 122, 44);
            btnYtDlpDownload.Click += btnYtDlpDownload_Click;
            panel.Controls.Add(btnYtDlpDownload);

            btnYtDlpCancel = CreateYtDlpButton("CANCEL", 528, 320, 122, 44);
            btnYtDlpCancel.Enabled = false;
            btnYtDlpCancel.Click += btnYtDlpCancel_Click;
            panel.Controls.Add(btnYtDlpCancel);

            lblYtDlpStatus = CreateYtDlpLabel("Waiting for input", 16, 382, 634);
            lblYtDlpStatus.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            lblYtDlpStatus.BackColor = Color.FromArgb(39, 39, 39);
            lblYtDlpStatus.TextAlign = ContentAlignment.MiddleCenter;
            lblYtDlpStatus.Height = 42;
            panel.Controls.Add(lblYtDlpStatus);

            progressBarYtDlp = new ProgressBar
            {
                Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right,
                Location = new Point(16, 439),
                Size = new Size(634, 22),
                Style = ProgressBarStyle.Continuous,
                MarqueeAnimationSpeed = 30
            };
            panel.Controls.Add(progressBarYtDlp);

            lblYtDlpInfo = CreateYtDlpLabel("No video metadata loaded", 16, 478, 634);
            lblYtDlpInfo.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            lblYtDlpInfo.BackColor = Color.FromArgb(39, 39, 39);
            lblYtDlpInfo.Height = 60;
            lblYtDlpInfo.TextAlign = ContentAlignment.MiddleLeft;
            panel.Controls.Add(lblYtDlpInfo);

            btnYtDlpRetryFailed = CreateYtDlpButton("RETRY FAILED", 16, 548, 154, 35);
            btnYtDlpRetryFailed.Click += btnYtDlpRetryFailed_Click;
            panel.Controls.Add(btnYtDlpRetryFailed);

            btnYtDlpRemoveSelected = CreateYtDlpButton("REMOVE", 178, 548, 122, 35);
            btnYtDlpRemoveSelected.Click += btnYtDlpRemoveSelected_Click;
            panel.Controls.Add(btnYtDlpRemoveSelected);

            btnYtDlpOpenFolder = CreateYtDlpButton("OPEN FOLDER", 308, 548, 154, 35);
            btnYtDlpOpenFolder.Click += btnYtDlpOpenFolder_Click;
            panel.Controls.Add(btnYtDlpOpenFolder);

            listBoxYtDlpQueue = new ListBox
            {
                Anchor = AnchorStyles.Top | AnchorStyles.Bottom | AnchorStyles.Left | AnchorStyles.Right,
                BackColor = Color.FromArgb(39, 39, 39),
                BorderStyle = BorderStyle.FixedSingle,
                ForeColor = Color.FromArgb(207, 210, 214),
                HorizontalScrollbar = true,
                IntegralHeight = false,
                Location = new Point(16, 591),
                Size = new Size(634, 58)
            };
            listBoxYtDlpQueue.SelectedIndexChanged += listBoxYtDlpQueue_SelectedIndexChanged;
            listBoxYtDlpQueue.DoubleClick += listBoxYtDlpQueue_DoubleClick;
            panel.Controls.Add(listBoxYtDlpQueue);

            txtYtDlpLog = new TextBox
            {
                Anchor = AnchorStyles.Bottom | AnchorStyles.Left | AnchorStyles.Right,
                BackColor = Color.FromArgb(39, 39, 39),
                BorderStyle = BorderStyle.FixedSingle,
                ForeColor = Color.FromArgb(207, 210, 214),
                Location = new Point(16, 658),
                Multiline = true,
                ReadOnly = true,
                ScrollBars = ScrollBars.Vertical,
                Size = new Size(634, 61)
            };
            panel.Controls.Add(txtYtDlpLog);
            ShiftYtDlpControlsBelow(panel, 58, 48);
            UpdateYtDlpQueueButtons();
            cmboxYtDlpFormatType_SelectedIndexChanged(this, EventArgs.Empty);
            ResizeYtDlpQueueAndLog(panel);
        }

        private void ConfigureMainWindowForTabs()
        {
            var workingArea = Screen.FromControl(this).WorkingArea;
            var desiredClientSize = new Size(760, 800);
            var minimumClientSize = new Size(
                Math.Min(700, Math.Max(560, workingArea.Width - 80)),
                Math.Min(620, Math.Max(520, workingArea.Height - 80)));
            ClientSize = new Size(
                Math.Min(desiredClientSize.Width, Math.Max(minimumClientSize.Width, workingArea.Width - 80)),
                Math.Min(desiredClientSize.Height, Math.Max(minimumClientSize.Height, workingArea.Height - 80)));
            MinimumSize = SizeFromClientSize(minimumClientSize);
            FormBorderStyle = FormBorderStyle.Sizable;
            MaximizeBox = true;
        }

        private void ConfigureLinkedInTabLayout(TabPage linkedInTab)
        {
            panelBody.Anchor = AnchorStyles.Top | AnchorStyles.Bottom | AnchorStyles.Left | AnchorStyles.Right;
            panelBody.AutoScroll = true;
            panelBody.MinimumSize = new Size(697, 728);
            panelBody.Size = new Size(
                Math.Max(panelBody.MinimumSize.Width, linkedInTab.ClientSize.Width - 16),
                Math.Max(panelBody.MinimumSize.Height, linkedInTab.ClientSize.Height - 16));

            panelInput.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            panelStatus.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;

            cmboxQuality.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            txtToken.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            cmboxBrowser.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            txtCourseDirectory.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            btnBrowse.Anchor = AnchorStyles.Top | AnchorStyles.Right;
            txtCourseUrls.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            btnDownload.Anchor = AnchorStyles.Top | AnchorStyles.Right;

            lblCurrentExtractionOperation.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            progressBarExtractor.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            progressBarCourses.Anchor = AnchorStyles.Top | AnchorStyles.Left | AnchorStyles.Right;
            label8.AutoSize = false;
            label10.AutoSize = false;
            UCCourseDownloaderStatus.Anchor = AnchorStyles.Top | AnchorStyles.Right;

            checkBoxDelay.AutoSize = false;
            checkBoxDelay.Size = new Size(316, 32);
            checkBoxVideos.AutoSize = false;
            checkBoxVideos.Size = new Size(220, 32);
            checkBoxExerciseFiles.AutoSize = false;
            checkBoxExerciseFiles.Size = new Size(250, 32);
            checkBoxSubtitles.AutoSize = false;
            checkBoxSubtitles.Size = new Size(220, 32);
            label2.AutoSize = false;
            label2.Size = new Size(110, 32);

            linkedInTab.Resize += linkedInTab_Resize;
            ResizeLinkedInTab(linkedInTab);
        }

        private void linkedInTab_Resize(object sender, EventArgs e)
        {
            ResizeLinkedInTab((TabPage)sender);
        }

        private void ResizeLinkedInTab(TabPage linkedInTab)
        {
            if (linkedInTab == null || panelBody == null || panelInput == null || panelStatus == null)
                return;

            int panelWidth = Math.Max(697, linkedInTab.ClientSize.Width - 16);
            panelBody.Size = new Size(panelWidth, Math.Max(728, linkedInTab.ClientSize.Height - 16));

            int innerWidth = Math.Max(671, panelBody.ClientSize.Width - 26);
            panelInput.Width = innerWidth;
            panelStatus.Width = innerWidth;

            int contentWidth = Math.Max(220, innerWidth - 203);
            cmboxQuality.Width = contentWidth;
            txtToken.Width = contentWidth;
            txtCourseUrls.Width = contentWidth;
            txtCourseDirectory.Width = Math.Max(180, innerWidth - 249);
            btnBrowse.Left = innerWidth - btnBrowse.Width - 21;

            cmboxBrowser.Width = Math.Max(170, innerWidth - cmboxBrowser.Left - 21);
            btnDownload.Left = innerWidth - btnDownload.Width - 21;

            label8.SetBounds(16, label8.Top, 170, 32);
            UC_CourseExtractorStatus.Left = label8.Right + 8;
            label10.SetBounds(Math.Max(328, innerWidth - UCCourseDownloaderStatus.Width - 220), label10.Top, 190, 32);
            UCCourseDownloaderStatus.Left = innerWidth - UCCourseDownloaderStatus.Width - 21;

            int wideStatusWidth = Math.Max(220, innerWidth - 37);
            lblCurrentExtractionOperation.Width = wideStatusWidth;
            progressBarExtractor.Width = wideStatusWidth;
            progressBarCourses.Width = wideStatusWidth;
        }

        private void RefreshResponsiveLayouts()
        {
            if (tabControlMain == null)
                return;

            foreach (TabPage tabPage in tabControlMain.TabPages)
            {
                if (tabPage.Text == "LinkedIn Learning")
                {
                    ResizeLinkedInTab(tabPage);
                }
            }

            if (txtYtDlpLog?.Parent is Panel ytDlpPanel)
            {
                ResizeYtDlpQueueAndLog(ytDlpPanel);
            }
        }

        private void panelYtDlp_Resize(object sender, EventArgs e)
        {
            ResizeYtDlpQueueAndLog((Panel)sender);
        }

        private void ResizeYtDlpQueueAndLog(Panel panel)
        {
            if (panel == null || listBoxYtDlpQueue == null || txtYtDlpLog == null)
                return;

            const int margin = 16;
            const int gap = 9;
            const int logMinHeight = 70;
            const int queueMinHeight = 58;
            int availableBottom = Math.Max(767, panel.ClientSize.Height - margin);
            int logHeight = Math.Max(logMinHeight, Math.Min(160, availableBottom - txtYtDlpLog.Top));
            txtYtDlpLog.Height = logHeight;
            txtYtDlpLog.Top = availableBottom - logHeight;

            int queueBottom = txtYtDlpLog.Top - gap;
            listBoxYtDlpQueue.Height = Math.Max(queueMinHeight, queueBottom - listBoxYtDlpQueue.Top);
        }

        private static void ShiftYtDlpControlsBelow(Control parent, int minimumTop, int deltaY)
        {
            foreach (Control control in parent.Controls)
            {
                if (control.Top >= minimumTop)
                {
                    control.Top += deltaY;
                }
            }
        }

        private async void btnYtDlpCheckDependencies_Click(object sender, EventArgs e)
        {
            CheckYtDlpDependencies();
            await TryUpdateYtDlpVersion();
        }

        private async void btnYtDlpInstallTools_Click(object sender, EventArgs e)
        {
            var confirm = MessageBox.Show(
                "Download yt-dlp and FFmpeg into the app tools folder?\n\nThis downloads executables from the upstream release locations and stores them next to this app.",
                "Download tools",
                MessageBoxButtons.YesNo,
                MessageBoxIcon.Question);
            if (confirm != DialogResult.Yes)
                return;

            _ytDlpCancellationTokenSource = new CancellationTokenSource();
            SetYtDlpBusy(true, "Downloading tools...");
            progressBarYtDlp.Style = ProgressBarStyle.Marquee;
            try
            {
                var installer = new YtDlpDependencyInstaller();
                var result = await installer.InstallAllAsync(
                    AppDomain.CurrentDomain.BaseDirectory,
                    _ytDlpCancellationTokenSource.Token,
                    progress => UpdateYtDlpInstallProgress(progress));

                AppendYtDlpLog("Installed yt-dlp: " + result.YtDlpPath);
                AppendYtDlpLog("Installed ffmpeg: " + result.FfmpegPath);
                CheckYtDlpDependencies();
                await TryUpdateYtDlpVersion();
                lblYtDlpStatus.Text = "Tools installed";
            }
            catch (OperationCanceledException)
            {
                lblYtDlpStatus.Text = "Tool download cancelled";
            }
            catch (Exception ex)
            {
                lblYtDlpStatus.Text = "Tool download failed";
                AppendYtDlpLog(ex.Message);
                MessageBox.Show(ex.Message, "Tool download failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
                Log.Error(ex, "yt-dlp tool install failed");
            }
            finally
            {
                progressBarYtDlp.Style = ProgressBarStyle.Continuous;
                _ytDlpCancellationTokenSource?.Dispose();
                _ytDlpCancellationTokenSource = null;
                SetYtDlpBusy(false);
            }
        }

        private async void btnYtDlpFetchInfo_Click(object sender, EventArgs e)
        {
            if (!ValidateYtDlpInput(requireFolder: false))
                return;

            if (!EnsureYtDlpDependencies(requireDownloadTools: false))
                return;

            _ytDlpCancellationTokenSource = new CancellationTokenSource();
            SetYtDlpBusy(true, "Fetching video metadata...");
            try
            {
                var service = CreateYtDlpService();
                var cancellationToken = _ytDlpCancellationTokenSource.Token;
                var inputUrls = GetYtDlpInputUrls();
                if (inputUrls.Count > 1)
                {
                    PopulateYtDlpQueueFromUrls(inputUrls, inputUrls);
                    ResetYtDlpFormats();
                    _ytDlpInfo = null;
                    lblYtDlpStatus.Text = $"{inputUrls.Count} URLs queued";
                    if (checkBoxYtDlpPlaylist.Checked)
                    {
                        AppendYtDlpLog("Playlist expansion is only applied when one URL is entered. Multiple URLs were queued directly.");
                    }
                }
                else if (checkBoxYtDlpPlaylist.Checked)
                {
                    var playlistInfo = await GetYtDlpPlaylistInfoWithCookieFallback(service, inputUrls[0], cancellationToken);
                    PopulateYtDlpQueue(playlistInfo, inputUrls, playlistExpanded: true);
                    _ytDlpInfo = playlistInfo.Entries.Count == 1
                        ? await GetYtDlpInfoWithCookieFallback(service, playlistInfo.Entries[0].Url, cancellationToken)
                        : null;
                    if (_ytDlpInfo != null)
                    {
                        PopulateYtDlpFormats(_ytDlpInfo);
                    }
                    else
                    {
                        ResetYtDlpFormats();
                    }
                }
                else
                {
                    _ytDlpInfo = await GetYtDlpInfoWithCookieFallback(service, inputUrls[0], cancellationToken);
                    PopulateYtDlpInfo(_ytDlpInfo);
                    PopulateYtDlpQueue(YtDlpPlaylistInfo.FromJson($@"{{ ""title"": ""{EscapeJson(_ytDlpInfo.Title ?? "Video")}"", ""webpage_url"": ""{EscapeJson(inputUrls[0])}"" }}"), inputUrls, playlistExpanded: false);
                }
                if (inputUrls.Count == 1)
                {
                    lblYtDlpStatus.Text = "Metadata loaded";
                }
            }
            catch (OperationCanceledException)
            {
                lblYtDlpStatus.Text = "Metadata fetch cancelled";
            }
            catch (Exception ex)
            {
                lblYtDlpStatus.Text = "Metadata fetch failed";
                AppendYtDlpLog(ex.Message);
                MessageBox.Show(ex.Message, "yt-dlp metadata failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
                Log.Error(ex, "yt-dlp metadata fetch failed");
            }
            finally
            {
                _ytDlpCancellationTokenSource?.Dispose();
                _ytDlpCancellationTokenSource = null;
                SetYtDlpBusy(false);
            }
        }

        private async void btnYtDlpDownload_Click(object sender, EventArgs e)
        {
            if (!ValidateYtDlpInput(requireFolder: true))
                return;

            if (!EnsureYtDlpDependencies(requireDownloadTools: true))
                return;

            Directory.CreateDirectory(txtYtDlpDirectory.Text.Trim());
            progressBarYtDlp.Style = ProgressBarStyle.Continuous;
            progressBarYtDlp.Value = 0;
            txtYtDlpLog.Clear();
            _ytDlpCancellationTokenSource = new CancellationTokenSource();
            SetYtDlpBusy(true, "Starting download...");

            try
            {
                await EnsureYtDlpQueueFromCurrentInput(CreateYtDlpService(), _ytDlpCancellationTokenSource.Token);
                var runner = new YtDlpJobRunner(CreateYtDlpService());
                var jobsToRun = _ytDlpQueue
                    .Where(job => job.Status != YtDlpJobStatus.Finished)
                    .ToList();
                if (jobsToRun.Count == 0)
                {
                    lblYtDlpStatus.Text = "No unfinished queue items";
                    progressBarYtDlp.Value = 0;
                    return;
                }

                int completed = 0;
                int failed = 0;
                bool cancelled = false;
                foreach (var job in jobsToRun)
                {
                    if (_ytDlpCancellationTokenSource.Token.IsCancellationRequested)
                    {
                        cancelled = true;
                        break;
                    }

                    job.Options = BuildYtDlpDownloadOptions(job.Url);
                    job.OutputTemplate = job.Options.OutputTemplate;
                    RefreshYtDlpQueueList();
                    lblYtDlpStatus.Text = $"Downloading {completed + failed + 1}/{jobsToRun.Count}";
                    var result = await DownloadYtDlpJobWithCookieFallback(runner, job, _ytDlpCancellationTokenSource.Token);
                    if (result.Success)
                    {
                        completed++;
                    }
                    else
                    {
                        failed++;
                    }
                    RefreshYtDlpQueueList();
                }

                await SaveYtDlpConfig();
                progressBarYtDlp.Style = ProgressBarStyle.Continuous;
                progressBarYtDlp.Value = cancelled ? 0 : progressBarYtDlp.Maximum;
                if (cancelled)
                {
                    lblYtDlpStatus.Text = "Download cancelled";
                    MessageBox.Show($"Download queue cancelled.\nCompleted: {completed}\nFailed: {failed}", "yt-dlp", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                }
                else
                {
                    lblYtDlpStatus.Text = failed == 0 ? "Download queue finished" : $"Download queue finished with {failed} failed";
                    MessageBox.Show($"Download queue finished.\nCompleted: {completed}\nFailed: {failed}", "yt-dlp", MessageBoxButtons.OK, failed == 0 ? MessageBoxIcon.Information : MessageBoxIcon.Warning);
                }
            }
            catch (OperationCanceledException)
            {
                lblYtDlpStatus.Text = "Download cancelled";
            }
            catch (Exception ex)
            {
                lblYtDlpStatus.Text = "Download failed";
                AppendYtDlpLog(ex.Message);
                MessageBox.Show(ex.Message, "yt-dlp download failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
                Log.Error(ex, "yt-dlp download failed");
            }
            finally
            {
                _ytDlpCancellationTokenSource?.Dispose();
                _ytDlpCancellationTokenSource = null;
                SetYtDlpBusy(false);
            }
        }

        private void btnYtDlpCancel_Click(object sender, EventArgs e)
        {
            _ytDlpCancellationTokenSource?.Cancel();
            lblYtDlpStatus.Text = "Cancelling...";
        }

        private void btnYtDlpRetryFailed_Click(object sender, EventArgs e)
        {
            int retryCount = 0;
            foreach (var job in _ytDlpQueue.Where(IsRetryableYtDlpJob))
            {
                job.ResetForRetry();
                retryCount++;
            }

            RefreshYtDlpQueueList();
            lblYtDlpStatus.Text = retryCount == 0
                ? "No failed or cancelled jobs to retry"
                : $"{retryCount} job(s) queued for retry";
        }

        private void btnYtDlpRemoveSelected_Click(object sender, EventArgs e)
        {
            int selectedIndex = listBoxYtDlpQueue.SelectedIndex;
            if (selectedIndex < 0 || selectedIndex >= _ytDlpQueue.Count)
            {
                lblYtDlpStatus.Text = "Select a queue item to remove";
                return;
            }

            var removed = _ytDlpQueue[selectedIndex];
            _ytDlpQueue.RemoveAt(selectedIndex);
            RefreshYtDlpQueueList();
            lblYtDlpInfo.Text = $"Queue\r\n{_ytDlpQueue.Count} item(s) queued";
            lblYtDlpStatus.Text = "Removed: " + (removed.Title ?? removed.Url);
        }

        private void btnYtDlpOpenFolder_Click(object sender, EventArgs e)
        {
            if (String.IsNullOrWhiteSpace(txtYtDlpDirectory.Text))
            {
                MessageBox.Show("Please choose a download folder first.", "Missing folder", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }

            string directory;
            try
            {
                directory = Path.GetFullPath(txtYtDlpDirectory.Text.Trim());
            }
            catch (Exception ex) when (ex is ArgumentException || ex is NotSupportedException || ex is PathTooLongException)
            {
                MessageBox.Show("Please choose a valid download folder.", "Invalid folder", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }

            if (!Directory.Exists(directory))
            {
                MessageBox.Show("The download folder does not exist yet.", "Folder missing", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return;
            }

            try
            {
                Process.Start(new ProcessStartInfo
                {
                    FileName = directory,
                    UseShellExecute = true
                });
            }
            catch (Exception ex)
            {
                AppendYtDlpLog("Could not open folder: " + ex.Message);
                MessageBox.Show(ex.Message, "Open folder failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
            }
        }

        private void listBoxYtDlpQueue_SelectedIndexChanged(object sender, EventArgs e)
        {
            UpdateYtDlpQueueButtons();
            ShowYtDlpSelectedJobSummary(false);
        }

        private void listBoxYtDlpQueue_DoubleClick(object sender, EventArgs e)
        {
            ShowYtDlpSelectedJobSummary(true);
        }

        private void btnYtDlpBrowse_Click(object sender, EventArgs e)
        {
            folderBrowserDialog.ShowDialog();
            txtYtDlpDirectory.Text = folderBrowserDialog.SelectedPath;
        }

        private void cmboxYtDlpFormatType_SelectedIndexChanged(object sender, EventArgs e)
        {
            bool audioMode = cmboxYtDlpFormatType.SelectedIndex == 1;
            cmboxYtDlpAudioFormat.Enabled = audioMode;
            cmboxYtDlpVideoFormat.Enabled = !audioMode;
        }

        private bool ValidateYtDlpInput(bool requireFolder)
        {
            var inputUrls = GetYtDlpInputUrls();
            if (inputUrls.Count == 0)
            {
                MessageBox.Show("Please enter at least one video URL.", "Missing URL", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return false;
            }
            foreach (var inputUrl in inputUrls)
            {
                if (!Uri.TryCreate(inputUrl, UriKind.Absolute, out var uri) ||
                    (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps))
                {
                    MessageBox.Show("Please enter valid http or https URLs only.", "Invalid URL", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                    return false;
                }
            }

            if (requireFolder && String.IsNullOrWhiteSpace(txtYtDlpDirectory.Text))
            {
                MessageBox.Show("Please choose a download folder.", "Missing folder", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return false;
            }
            if (requireFolder)
            {
                try
                {
                    Path.GetFullPath(txtYtDlpDirectory.Text.Trim());
                }
                catch (Exception ex) when (ex is ArgumentException || ex is NotSupportedException || ex is PathTooLongException)
                {
                    MessageBox.Show("Please choose a valid download folder.", "Invalid folder", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                    return false;
                }
            }

            return true;
        }

        private List<string> GetYtDlpInputUrls()
        {
            return YtDlpUrlListParser.Parse(txtYtDlpUrl.Text);
        }

        private bool EnsureYtDlpDependencies(bool requireDownloadTools)
        {
            if (_ytDlpDependencies == null)
            {
                CheckYtDlpDependencies();
            }

            if (!_ytDlpDependencies.HasYtDlp)
            {
                AppendYtDlpLog("Install hint: python -m pip install -U yt-dlp");
                MessageBox.Show("yt-dlp was not found on PATH. Install yt-dlp or add it to PATH, then check tools again.", "yt-dlp missing", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return false;
            }

            if (requireDownloadTools && !_ytDlpDependencies.CanRun(BuildYtDlpDownloadOptionsPreview()))
            {
                AppendYtDlpLog("Install hint: install ffmpeg and add its bin folder to PATH.");
                MessageBox.Show("ffmpeg is required for video merging and audio extraction.", "ffmpeg missing", MessageBoxButtons.OK, MessageBoxIcon.Warning);
                return false;
            }

            return true;
        }

        private YtDlpDownloadOptions BuildYtDlpDownloadOptionsPreview()
        {
            return new YtDlpDownloadOptions
            {
                FormatChoice = cmboxYtDlpFormatType.SelectedIndex == 1 ? YtDlpFormatChoice.Audio : YtDlpFormatChoice.Video
            };
        }

        private void CheckYtDlpDependencies()
        {
            _ytDlpDependencies = YtDlpDependencyChecker.Check();
            AppendYtDlpLog("yt-dlp: " + (_ytDlpDependencies.YtDlpPath ?? "not found"));
            AppendYtDlpLog("ffmpeg: " + (_ytDlpDependencies.FfmpegPath ?? "not found"));
            lblYtDlpStatus.Text = _ytDlpDependencies.HasYtDlp ? "yt-dlp is available" : "yt-dlp is missing";
        }

        private async Task TryUpdateYtDlpVersion()
        {
            if (_ytDlpDependencies == null || !_ytDlpDependencies.HasYtDlp)
                return;

            try
            {
                var version = await CreateYtDlpService().GetVersion();
                AppendYtDlpLog("yt-dlp version: " + version);
            }
            catch (Exception ex)
            {
                AppendYtDlpLog("Could not read yt-dlp version: " + ex.Message);
            }
        }

        private YtDlpService CreateYtDlpService()
        {
            return new YtDlpService(_ytDlpDependencies?.YtDlpPath ?? "yt-dlp");
        }

        private void PopulateYtDlpInfo(YtDlpInfo info)
        {
            PopulateYtDlpFormats(info);
            var subtitleText = $"{info.Subtitles.Count} subtitles, {info.AutomaticCaptions.Count} auto captions";
            lblYtDlpInfo.Text = $"{info.Title ?? "Untitled"}\r\n{info.Uploader ?? "Unknown uploader"} | {subtitleText}";
        }

        private void PopulateYtDlpFormats(YtDlpInfo info)
        {
            _ytDlpFormats = info.Formats ?? new List<YtDlpFormat>();
            cmboxYtDlpVideoFormat.Items.Clear();
            cmboxYtDlpVideoFormat.Items.Add("Best available");
            foreach (var format in _ytDlpFormats)
            {
                cmboxYtDlpVideoFormat.Items.Add($"{format.Label} ({format.Id})");
            }
            cmboxYtDlpVideoFormat.SelectedIndex = 0;
        }

        private void ResetYtDlpFormats()
        {
            _ytDlpFormats = new List<YtDlpFormat>();
            cmboxYtDlpVideoFormat.Items.Clear();
            cmboxYtDlpVideoFormat.Items.Add("Best available");
            cmboxYtDlpVideoFormat.SelectedIndex = 0;
        }

        private void PopulateYtDlpQueue(YtDlpPlaylistInfo playlistInfo, IEnumerable<string> sourceUrls, bool playlistExpanded)
        {
            _ytDlpQueue = playlistInfo.Entries
                .GroupBy(entry => entry.Url, StringComparer.OrdinalIgnoreCase)
                .Select(group => group.First())
                .Select(entry => new YtDlpJob
                {
                    Url = entry.Url,
                    Title = entry.Title,
                    Status = YtDlpJobStatus.Queued
                })
                .ToList();
            _ytDlpQueueSourceKey = BuildYtDlpQueueSourceKey(sourceUrls, playlistExpanded);

            RefreshYtDlpQueueList();
            lblYtDlpInfo.Text = $"{playlistInfo.Title ?? "Queue"}\r\n{_ytDlpQueue.Count} item(s) queued";
        }

        private void PopulateYtDlpQueueFromUrls(IEnumerable<string> urls, IEnumerable<string> sourceUrls = null)
        {
            _ytDlpQueue = urls
                .Where(url => !String.IsNullOrWhiteSpace(url))
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .Select((url, index) => new YtDlpJob
                {
                    Url = url,
                    Title = "URL " + (index + 1),
                    Status = YtDlpJobStatus.Queued
                })
                .ToList();
            _ytDlpQueueSourceKey = BuildYtDlpQueueSourceKey(sourceUrls ?? urls, playlistExpanded: false);

            RefreshYtDlpQueueList();
            lblYtDlpInfo.Text = $"Queue\r\n{_ytDlpQueue.Count} URL(s) queued";
        }

        private async Task EnsureYtDlpQueueFromCurrentInput(YtDlpService service, CancellationToken cancellationToken)
        {
            var inputUrls = GetYtDlpInputUrls();
            bool playlistExpansionRequested = checkBoxYtDlpPlaylist.Checked && inputUrls.Count == 1;
            var sourceKey = BuildYtDlpQueueSourceKey(inputUrls, playlistExpansionRequested);
            if (_ytDlpQueue.Count > 0 && String.Equals(_ytDlpQueueSourceKey, sourceKey, StringComparison.Ordinal))
                return;

            if (inputUrls.Count > 1)
            {
                PopulateYtDlpQueueFromUrls(inputUrls, inputUrls);
                return;
            }

            if (checkBoxYtDlpPlaylist.Checked)
            {
                lblYtDlpStatus.Text = "Expanding playlist...";
                var playlistInfo = await GetYtDlpPlaylistInfoWithCookieFallback(service, inputUrls[0], cancellationToken);
                PopulateYtDlpQueue(playlistInfo, inputUrls, playlistExpanded: true);
                return;
            }

            _ytDlpQueue = new List<YtDlpJob>
            {
                new YtDlpJob
                {
                    Url = inputUrls[0],
                    Title = _ytDlpInfo?.Title,
                    Status = YtDlpJobStatus.Queued
                }
            };
            _ytDlpQueueSourceKey = sourceKey;
            RefreshYtDlpQueueList();
        }

        private static string BuildYtDlpQueueSourceKey(IEnumerable<string> urls, bool playlistExpanded)
        {
            if (urls == null)
                return String.Empty;

            return (playlistExpanded ? "playlist" : "direct") + "\n" + String.Join("\n", urls
                .Where(url => !String.IsNullOrWhiteSpace(url))
                .Select(url => url.Trim().ToLowerInvariant()));
        }

        private void RefreshYtDlpQueueList()
        {
            int selectedIndex = listBoxYtDlpQueue.SelectedIndex;
            listBoxYtDlpQueue.Items.Clear();
            foreach (var job in _ytDlpQueue)
            {
                var label = $"{job.Status}: {job.Title ?? job.Url}";
                if (job.Status == YtDlpJobStatus.Finished && !String.IsNullOrWhiteSpace(job.OutputFileName))
                {
                    label += " -> " + job.OutputFileName;
                }
                if (job.Status == YtDlpJobStatus.Failed && !String.IsNullOrWhiteSpace(job.ErrorMessage))
                {
                    label += " - " + FirstLine(job.ErrorMessage);
                }
                listBoxYtDlpQueue.Items.Add(label);
            }

            if (selectedIndex >= 0 && selectedIndex < listBoxYtDlpQueue.Items.Count)
            {
                listBoxYtDlpQueue.SelectedIndex = selectedIndex;
            }

            UpdateYtDlpQueueButtons();
        }

        private YtDlpDownloadOptions BuildYtDlpDownloadOptions(string url = null, YtDlpBrowserCookiesSource? cookiesSource = null)
        {
            var options = new YtDlpDownloadOptions
            {
                Url = String.IsNullOrWhiteSpace(url) ? txtYtDlpUrl.Text.Trim() : url,
                OutputTemplate = BuildYtDlpOutputTemplate(),
                FfmpegLocation = GetFfmpegLocation(),
                FormatChoice = cmboxYtDlpFormatType.SelectedIndex == 1 ? YtDlpFormatChoice.Audio : YtDlpFormatChoice.Video,
                AudioFormat = GetSelectedYtDlpAudioFormat(),
                CookiesSource = cookiesSource ?? GetSelectedYtDlpCookies(),
                WriteSubtitles = checkBoxYtDlpSubtitles.Checked,
                WriteAutomaticSubtitles = checkBoxYtDlpAutoSubtitles.Checked,
                SubtitleLanguages = txtYtDlpSubtitleLanguages.Text.Trim(),
                WriteInfoJson = checkBoxYtDlpInfoJson.Checked,
                WriteThumbnail = checkBoxYtDlpThumbnail.Checked
            };

            if (options.FormatChoice == YtDlpFormatChoice.Video && cmboxYtDlpVideoFormat.SelectedIndex > 0)
            {
                options.FormatId = _ytDlpFormats[cmboxYtDlpVideoFormat.SelectedIndex - 1].Id;
            }

            return options;
        }

        private async Task<YtDlpInfo> GetYtDlpInfoWithCookieFallback(YtDlpService service, string url, CancellationToken cancellationToken)
        {
            var cookiesSource = GetSelectedYtDlpCookies();
            try
            {
                return await service.GetInfo(url, cookiesSource, cancellationToken);
            }
            catch (Exception ex) when (ShouldRetryWithoutBrowserCookies(cookiesSource, ex))
            {
                WarnAndDisableYtDlpBrowserCookies(ex);
                return await service.GetInfo(url, YtDlpBrowserCookiesSource.None, cancellationToken);
            }
        }

        private async Task<YtDlpPlaylistInfo> GetYtDlpPlaylistInfoWithCookieFallback(YtDlpService service, string url, CancellationToken cancellationToken)
        {
            var cookiesSource = GetSelectedYtDlpCookies();
            try
            {
                return await service.GetPlaylistInfo(url, cookiesSource, cancellationToken);
            }
            catch (Exception ex) when (ShouldRetryWithoutBrowserCookies(cookiesSource, ex))
            {
                WarnAndDisableYtDlpBrowserCookies(ex);
                return await service.GetPlaylistInfo(url, YtDlpBrowserCookiesSource.None, cancellationToken);
            }
        }

        private async Task<YtDlpDownloadResult> DownloadYtDlpJobWithCookieFallback(YtDlpJobRunner runner, YtDlpJob job, CancellationToken cancellationToken)
        {
            var result = await runner.Download(job, cancellationToken, UpdateYtDlpProgress);
            if (result.Success || !ShouldRetryWithoutBrowserCookies(job.Options?.CookiesSource ?? YtDlpBrowserCookiesSource.None, result.Error))
            {
                return result;
            }

            WarnAndDisableYtDlpBrowserCookies(result.Error);
            job.ResetForRetry();
            job.Logs.Add("Retrying without browser cookies after cookie database copy failure.");
            job.Options = BuildYtDlpDownloadOptions(job.Url, YtDlpBrowserCookiesSource.None);
            job.OutputTemplate = job.Options.OutputTemplate;
            RefreshYtDlpQueueList();
            return await runner.Download(job, cancellationToken, UpdateYtDlpProgress);
        }

        private bool ShouldRetryWithoutBrowserCookies(YtDlpBrowserCookiesSource cookiesSource, Exception exception)
        {
            return exception != null && ShouldRetryWithoutBrowserCookies(cookiesSource, exception.Message);
        }

        private static bool ShouldRetryWithoutBrowserCookies(YtDlpBrowserCookiesSource cookiesSource, string message)
        {
            return cookiesSource != YtDlpBrowserCookiesSource.None &&
                   !String.IsNullOrWhiteSpace(message) &&
                   message.IndexOf("Could not copy", StringComparison.OrdinalIgnoreCase) >= 0 &&
                   message.IndexOf("cookie database", StringComparison.OrdinalIgnoreCase) >= 0;
        }

        private void WarnAndDisableYtDlpBrowserCookies(object detail)
        {
            AppendYtDlpLog("Browser cookies are locked, likely because the browser is open. Retrying without browser cookies.");
            AppendYtDlpLog("For public YouTube videos, use Browser cookies: None. For login-only videos, close the browser and retry with cookies.");
            if (detail != null)
            {
                AppendYtDlpLog(detail.ToString());
            }
            SetComboBoxIndex(cmboxYtDlpCookies, 0);
        }

        private string BuildYtDlpOutputTemplate()
        {
            return Path.Combine(txtYtDlpDirectory.Text.Trim(), "%(title)s.%(ext)s");
        }

        private string GetFfmpegLocation()
        {
            if (_ytDlpDependencies == null || String.IsNullOrWhiteSpace(_ytDlpDependencies.FfmpegPath))
                return null;

            return _ytDlpDependencies.FfmpegPath;
        }

        private YtDlpBrowserCookiesSource GetSelectedYtDlpCookies()
        {
            switch (cmboxYtDlpCookies.SelectedIndex)
            {
                case 1:
                    return YtDlpBrowserCookiesSource.Chrome;
                case 2:
                    return YtDlpBrowserCookiesSource.Firefox;
                case 3:
                    return YtDlpBrowserCookiesSource.Edge;
                default:
                    return YtDlpBrowserCookiesSource.None;
            }
        }

        private YtDlpAudioFormat GetSelectedYtDlpAudioFormat()
        {
            switch (cmboxYtDlpAudioFormat.SelectedIndex)
            {
                case 1:
                    return YtDlpAudioFormat.M4a;
                case 2:
                    return YtDlpAudioFormat.Flac;
                case 3:
                    return YtDlpAudioFormat.Wav;
                default:
                    return YtDlpAudioFormat.Mp3;
            }
        }

        private void UpdateYtDlpProgress(YtDlpProgress progress)
        {
            UpdateUI(() =>
            {
                if (progress.Percent.HasValue)
                {
                    progressBarYtDlp.Style = ProgressBarStyle.Continuous;
                    progressBarYtDlp.Value = Math.Max(0, Math.Min(progressBarYtDlp.Maximum, (int)Math.Round(progress.Percent.Value)));
                }
                else if (progress.Status == YtDlpJobStatus.Converting)
                {
                    progressBarYtDlp.Style = ProgressBarStyle.Marquee;
                }

                lblYtDlpStatus.Text = progress.Percent.HasValue
                    ? $"{progress.Message}: {progress.Percent.Value:0.0}%"
                    : progress.Message ?? progress.RawLine;
                AppendYtDlpLog(progress.RawLine);
            });
        }

        private void UpdateYtDlpInstallProgress(YtDlpDependencyInstallProgress progress)
        {
            if (progress == null)
                return;

            UpdateUI(() =>
            {
                if (progress.Percent.HasValue)
                {
                    progressBarYtDlp.Style = ProgressBarStyle.Continuous;
                    progressBarYtDlp.Value = Math.Max(0, Math.Min(progressBarYtDlp.Maximum, (int)Math.Round(progress.Percent.Value)));
                    lblYtDlpStatus.Text = $"{progress.Message}: {progress.Percent.Value:0.0}%";
                }
                else
                {
                    progressBarYtDlp.Style = ProgressBarStyle.Marquee;
                    lblYtDlpStatus.Text = progress.Message ?? "Downloading tools...";
                }
            });
        }

        private void SetYtDlpBusy(bool isBusy, string status = null)
        {
            _isYtDlpBusy = isBusy;
            btnYtDlpCheckDependencies.Enabled = !isBusy;
            btnYtDlpInstallTools.Enabled = !isBusy;
            btnYtDlpFetchInfo.Enabled = !isBusy;
            btnYtDlpDownload.Enabled = !isBusy;
            btnYtDlpCancel.Enabled = isBusy;
            UpdateYtDlpQueueButtons();
            if (!String.IsNullOrWhiteSpace(status))
            {
                lblYtDlpStatus.Text = status;
            }
        }

        private void UpdateYtDlpQueueButtons()
        {
            if (btnYtDlpRetryFailed == null || btnYtDlpRemoveSelected == null || btnYtDlpOpenFolder == null)
                return;

            btnYtDlpRetryFailed.Enabled = !_isYtDlpBusy && _ytDlpQueue.Any(IsRetryableYtDlpJob);
            btnYtDlpRemoveSelected.Enabled = !_isYtDlpBusy && listBoxYtDlpQueue != null && listBoxYtDlpQueue.SelectedIndex >= 0;
            btnYtDlpOpenFolder.Enabled = !_isYtDlpBusy;
        }

        private static bool IsRetryableYtDlpJob(YtDlpJob job)
        {
            return job != null && (job.Status == YtDlpJobStatus.Failed || job.Status == YtDlpJobStatus.Cancelled);
        }

        private void ShowYtDlpSelectedJobSummary(bool includeLogs)
        {
            if (listBoxYtDlpQueue == null || lblYtDlpInfo == null)
                return;

            int selectedIndex = listBoxYtDlpQueue.SelectedIndex;
            if (selectedIndex < 0 || selectedIndex >= _ytDlpQueue.Count)
                return;

            var job = _ytDlpQueue[selectedIndex];
            lblYtDlpInfo.Text = BuildYtDlpJobSummary(job);

            if (!includeLogs || txtYtDlpLog == null)
                return;

            txtYtDlpLog.Clear();
            AppendYtDlpLog("Selected queue item");
            AppendYtDlpLog(job.Title ?? job.Url);
            AppendYtDlpLog("Status: " + job.Status);
            if (!String.IsNullOrWhiteSpace(job.OutputFilePath))
            {
                AppendYtDlpLog("Output: " + job.OutputFilePath);
            }
            if (!String.IsNullOrWhiteSpace(job.ErrorMessage))
            {
                AppendYtDlpLog("Error: " + job.ErrorMessage);
            }
            AppendYtDlpLogLines(job.Logs);
            lblYtDlpStatus.Text = "Selected job details loaded";
        }

        private static string BuildYtDlpJobSummary(YtDlpJob job)
        {
            if (job == null)
                return "No queue item selected";

            var lines = new List<string>
            {
                job.Status + ": " + (job.Title ?? job.Url)
            };

            if (job.Progress?.Percent.HasValue == true)
            {
                lines.Add("Progress: " + job.Progress.Percent.Value.ToString("0.0") + "%");
            }
            else if (!String.IsNullOrWhiteSpace(job.Progress?.Message))
            {
                lines.Add("Progress: " + job.Progress.Message);
            }

            if (!String.IsNullOrWhiteSpace(job.OutputFileName))
            {
                lines.Add("Output: " + job.OutputFileName);
            }
            else if (!String.IsNullOrWhiteSpace(job.ErrorMessage))
            {
                lines.Add("Error: " + FirstLine(job.ErrorMessage));
            }
            else if (!String.IsNullOrWhiteSpace(job.Url) && !String.Equals(job.Title, job.Url, StringComparison.OrdinalIgnoreCase))
            {
                lines.Add(job.Url);
            }

            return String.Join(Environment.NewLine, lines.Take(3));
        }

        private static string FirstLine(string text)
        {
            if (String.IsNullOrWhiteSpace(text))
                return String.Empty;

            return text.Split(new[] { "\r\n", "\r", "\n" }, StringSplitOptions.None)[0];
        }

        private void AppendYtDlpLog(string line)
        {
            if (String.IsNullOrWhiteSpace(line) || txtYtDlpLog == null)
                return;

            txtYtDlpLog.AppendText(line + Environment.NewLine);
        }

        private void AppendYtDlpLogLines(IEnumerable<string> lines)
        {
            if (lines == null)
                return;

            foreach (var line in lines)
            {
                AppendYtDlpLog(line);
            }
        }

        private async Task SaveYtDlpConfig()
        {
            var config = await LoadConfigOrDefault();
            ApplyYtDlpSettingsToConfig(config);
            await config.Save();
        }

        private void ApplyYtDlpSettingsToConfig(Config config)
        {
            if (config == null || txtYtDlpDirectory == null)
                return;

            if (!String.IsNullOrWhiteSpace(txtYtDlpDirectory.Text))
            {
                try
                {
                    config.YtDlpDownloadDirectory = new DirectoryInfo(txtYtDlpDirectory.Text.Trim());
                }
                catch (ArgumentException)
                {
                    config.YtDlpDownloadDirectory = null;
                }
            }

            config.YtDlpCookiesSourceIndex = cmboxYtDlpCookies?.SelectedIndex ?? 0;
            config.YtDlpFormatTypeIndex = cmboxYtDlpFormatType?.SelectedIndex ?? 0;
            config.YtDlpAudioFormatIndex = cmboxYtDlpAudioFormat?.SelectedIndex ?? 0;
            config.YtDlpDownloadSubtitles = checkBoxYtDlpSubtitles?.Checked ?? false;
            config.YtDlpDownloadAutoSubtitles = checkBoxYtDlpAutoSubtitles?.Checked ?? false;
            config.YtDlpWriteInfoJson = checkBoxYtDlpInfoJson?.Checked ?? false;
            config.YtDlpWriteThumbnail = checkBoxYtDlpThumbnail?.Checked ?? false;
            config.YtDlpAutoDetectPlaylist = checkBoxYtDlpPlaylist?.Checked ?? false;
            config.YtDlpSubtitleLanguages = String.IsNullOrWhiteSpace(txtYtDlpSubtitleLanguages?.Text)
                ? "en"
                : txtYtDlpSubtitleLanguages.Text.Trim();
        }

        private void ApplyYtDlpSettingsFromConfig(Config config)
        {
            if (config == null || txtYtDlpDirectory == null)
                return;

            if (config.YtDlpDownloadDirectory != null)
            {
                txtYtDlpDirectory.Text = config.YtDlpDownloadDirectory.FullName;
            }
            SetComboBoxIndex(cmboxYtDlpCookies, config.YtDlpCookiesSourceIndex);
            SetComboBoxIndex(cmboxYtDlpFormatType, config.YtDlpFormatTypeIndex);
            SetComboBoxIndex(cmboxYtDlpAudioFormat, config.YtDlpAudioFormatIndex);
            checkBoxYtDlpSubtitles.Checked = config.YtDlpDownloadSubtitles;
            checkBoxYtDlpAutoSubtitles.Checked = config.YtDlpDownloadAutoSubtitles;
            checkBoxYtDlpInfoJson.Checked = config.YtDlpWriteInfoJson;
            checkBoxYtDlpThumbnail.Checked = config.YtDlpWriteThumbnail;
            checkBoxYtDlpPlaylist.Checked = config.YtDlpAutoDetectPlaylist;
            txtYtDlpSubtitleLanguages.Text = String.IsNullOrWhiteSpace(config.YtDlpSubtitleLanguages)
                ? "en"
                : config.YtDlpSubtitleLanguages;
        }

        private static string EscapeJson(string value)
        {
            if (String.IsNullOrEmpty(value))
                return String.Empty;

            return value.Replace("\\", "\\\\").Replace("\"", "\\\"");
        }

        private static void SetComboBoxIndex(ComboBox comboBox, int index)
        {
            if (comboBox == null || comboBox.Items.Count == 0)
                return;

            comboBox.SelectedIndex = index >= 0 && index < comboBox.Items.Count ? index : 0;
        }

        private static Label CreateYtDlpLabel(string text, int x, int y, int width)
        {
            return new Label
            {
                AutoSize = false,
                Font = new Font("Quicksand", 14.25F),
                ForeColor = Color.FromArgb(207, 210, 214),
                Location = new Point(x, y),
                Size = new Size(width, 31),
                Text = text
            };
        }

        private static TextBox CreateYtDlpTextBox(int x, int y, int width)
        {
            return new TextBox
            {
                BackColor = Color.FromArgb(53, 53, 53),
                BorderStyle = BorderStyle.FixedSingle,
                Font = new Font("Quicksand", 14.25F),
                ForeColor = Color.FromArgb(207, 210, 214),
                Location = new Point(x, y),
                Size = new Size(width, 31)
            };
        }

        private static ComboBox CreateYtDlpComboBox(int x, int y, int width)
        {
            return new ComboBox
            {
                BackColor = Color.FromArgb(53, 53, 53),
                DropDownStyle = ComboBoxStyle.DropDownList,
                FlatStyle = FlatStyle.Flat,
                Font = new Font("Quicksand", 14.25F),
                ForeColor = Color.FromArgb(207, 210, 214),
                Location = new Point(x, y),
                Size = new Size(width, 36)
            };
        }

        private static Button CreateYtDlpButton(string text, int x, int y, int width, int height)
        {
            return new Button
            {
                BackColor = Color.FromArgb(187, 134, 252),
                FlatStyle = FlatStyle.Flat,
                Font = new Font("Quicksand Medium", 12F, FontStyle.Bold),
                ForeColor = Color.Black,
                Location = new Point(x, y),
                Size = new Size(width, height),
                Text = text,
                UseVisualStyleBackColor = false
            };
        }

        private static CheckBox CreateYtDlpCheckBox(string text, int x, int y, int width)
        {
            return new CheckBox
            {
                AutoSize = false,
                Font = new Font("Quicksand", 14.25F),
                ForeColor = Color.FromArgb(207, 210, 214),
                Location = new Point(x, y),
                Size = new Size(width, 32),
                Text = text,
                UseVisualStyleBackColor = true
            };
        }
    }
}
