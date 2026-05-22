using System;
using System.IO;
using System.Reflection;
using System.Windows.Forms;
using LLCD.DownloaderConfig;
using Microsoft.Win32;
using Serilog;

namespace LLCD.DownloaderGUI
{
    static class Program
    {
        /// <summary>
        /// The main entry point for the application.
        /// </summary>
        [STAThread]
        static void Main()
        {
            AppDomain.CurrentDomain.UnhandledException += AllUnhandledExceptions;
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Log.Logger = new LoggerConfiguration()
                .MinimumLevel.Debug()
                .WriteTo.File("./logs/log.txt", rollingInterval: RollingInterval.Day, retainedFileCountLimit: 10)
                .CreateLogger();
            Application.SetUnhandledExceptionMode(UnhandledExceptionMode.ThrowException);
            if (!File.Exists("IconAddedToRegistry"))
            {
                AddIconToRegistry();
                using (File.Create("IconAddedToRegistry"))
                {
                }
            }
            Config.Restore();
            Application.Run(new MainForm());
        }
        private static void AddIconToRegistry()
        {
            Log.Information("Setting key value for icon in registry");
            string exePath = Assembly.GetExecutingAssembly().Location;
            using (RegistryKey baseKey = RegistryKey.OpenBaseKey(RegistryHive.CurrentUser, RegistryView.Registry64))
            using (RegistryKey myKey = baseKey.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Uninstall\Linkedin-Learning-Courses-Downloader", true))
            {
                Log.Information("Current exe path : " + exePath);
                if (myKey is null)
                {
                    Log.Warning("Key not found");
                }
                else
                {
                    myKey.SetValue("DisplayIcon", exePath, RegistryValueKind.String);
                    Log.Information("Key value for icon is set successfully");
                }
            }
        }
        private static void AllUnhandledExceptions(object sender, UnhandledExceptionEventArgs e)
        {
            var ex = (Exception)e.ExceptionObject;
            Log.Error(ex, "Unknown error occured");

            Environment.Exit(System.Runtime.InteropServices.Marshal.GetHRForException(ex));
        }
    }
}
