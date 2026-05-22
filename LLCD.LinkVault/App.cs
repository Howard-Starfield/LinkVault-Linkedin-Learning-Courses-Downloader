using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using Avalonia.Media;
using Avalonia.Styling;
using Avalonia.Themes.Fluent;

namespace LLCD.LinkVault;

public class App : Application
{
    public override void Initialize()
    {
        Styles.Add(new FluentTheme());
        RequestedThemeVariant = ThemeVariant.Dark;

        Resources["AccentBrush"] = new SolidColorBrush(Color.Parse("#8F7CFF"));
        Resources["PanelBrush"] = new SolidColorBrush(Color.Parse("#171923"));
        Resources["CardBrush"] = new SolidColorBrush(Color.Parse("#202432"));
        Resources["MutedBrush"] = new SolidColorBrush(Color.Parse("#9AA4B2"));
        Resources["BorderBrushSoft"] = new SolidColorBrush(Color.Parse("#343A4D"));
    }

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            desktop.MainWindow = new MainWindow();
        }

        base.OnFrameworkInitializationCompleted();
    }
}
