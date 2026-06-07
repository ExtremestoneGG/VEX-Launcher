using System;
using System.Drawing;
using System.IO;
using System.Threading.Tasks;
using System.Windows.Forms;
using Microsoft.Web.WebView2.Core;
using Microsoft.Web.WebView2.WinForms;

namespace VexMicrosoftAuth;

internal static class Program
{
    private const string RedirectUri = "https://login.live.com/oauth20_desktop.srf";

    [STAThread]
    private static void Main(string[] args)
    {
        if (args.Length != 2 || !Uri.TryCreate(args[0], UriKind.Absolute, out var loginUri) ||
            loginUri.Scheme != Uri.UriSchemeHttps || !loginUri.Host.Equals("login.live.com", StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        Application.Run(new LoginForm(loginUri, Path.GetFullPath(args[1])));
    }

    private sealed class LoginForm : Form
    {
        private readonly Uri _loginUri;
        private readonly string _resultPath;
        private readonly WebView2 _webView;
        private readonly Label _status;

        internal LoginForm(Uri loginUri, string resultPath)
        {
            _loginUri = loginUri;
            _resultPath = resultPath;
            Text = "Entrar com Microsoft - VEX Launcher";
            StartPosition = FormStartPosition.CenterScreen;
            MinimumSize = new Size(470, 620);
            ClientSize = new Size(540, 720);
            BackColor = Color.FromArgb(18, 23, 27);
            Icon = null;

            _status = new Label
            {
                Dock = DockStyle.Top,
                Height = 42,
                Text = "Abrindo a página oficial da Microsoft...",
                ForeColor = Color.FromArgb(200, 235, 255),
                BackColor = Color.FromArgb(18, 23, 27),
                TextAlign = ContentAlignment.MiddleCenter
            };

            _webView = new WebView2
            {
                Dock = DockStyle.Fill,
                DefaultBackgroundColor = Color.White
            };
            _webView.NavigationStarting += OnNavigationStarting;
            Controls.Add(_webView);
            Controls.Add(_status);
            Shown += async (_, _) => await InitializeBrowserAsync();
        }

        private async Task InitializeBrowserAsync()
        {
            try
            {
                var resultDirectory = Path.GetDirectoryName(_resultPath) ?? Path.GetTempPath();
                Directory.CreateDirectory(resultDirectory);
                var userDataDirectory = Path.Combine(resultDirectory, "microsoft-webview");
                var environment = await CoreWebView2Environment.CreateAsync(null, userDataDirectory);
                await _webView.EnsureCoreWebView2Async(environment);
                _webView.CoreWebView2.Settings.AreDevToolsEnabled = false;
                _webView.CoreWebView2.Settings.AreDefaultContextMenusEnabled = false;
                _status.Visible = false;
                _webView.CoreWebView2.Navigate(_loginUri.AbsoluteUri);
            }
            catch (Exception error)
            {
                _status.Text = $"Não foi possível abrir o login: {error.Message}";
                _status.ForeColor = Color.FromArgb(229, 114, 123);
            }
        }

        private void OnNavigationStarting(object? sender, CoreWebView2NavigationStartingEventArgs args)
        {
            if (!args.Uri.StartsWith(RedirectUri, StringComparison.OrdinalIgnoreCase))
            {
                return;
            }

            args.Cancel = true;
            try
            {
                File.WriteAllText(_resultPath, args.Uri);
            }
            finally
            {
                Close();
            }
        }
    }
}
