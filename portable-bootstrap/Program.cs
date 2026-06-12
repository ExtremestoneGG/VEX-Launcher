using System;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Windows.Forms;

[assembly: AssemblyTitle("VEX Launcher Portable")]
[assembly: AssemblyDescription("Self-contained portable bootstrap for the open-source VEX Minecraft Launcher")]
[assembly: AssemblyCompany("VEX Launcher")]
[assembly: AssemblyProduct("VEX Launcher")]
[assembly: AssemblyCopyright("Copyright (c) VEX Launcher contributors")]
[assembly: AssemblyVersion("0.9.0.0")]
[assembly: AssemblyFileVersion("0.9.0.0")]

internal static class Program
{
    private const string LauncherResource = "VexLauncher.Payload.exe";
    private const string LoaderResource = "VexLauncher.WebView2Loader.dll";

    [STAThread]
    private static void Main()
    {
        try
        {
            byte[] launcher = ReadResource(LauncherResource);
            byte[] loader = ReadResource(LoaderResource);
            string version = ShortHash(launcher);
            string runtimeDirectory = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "VEX Launcher",
                "portable-runtime",
                version);
            Directory.CreateDirectory(runtimeDirectory);

            string launcherPath = Path.Combine(runtimeDirectory, "vex-launcher.exe");
            string loaderPath = Path.Combine(runtimeDirectory, "WebView2Loader.dll");
            WriteIfChanged(launcherPath, launcher);
            WriteIfChanged(loaderPath, loader);

            ProcessStartInfo start = new ProcessStartInfo();
            start.FileName = launcherPath;
            start.WorkingDirectory = runtimeDirectory;
            start.Arguments = ForwardedArguments();
            start.UseShellExecute = true;
            Process.Start(start);
        }
        catch (Exception error)
        {
            MessageBox.Show(
                "Não foi possível abrir o VEX Launcher.\n\n" + error.Message,
                "VEX Launcher Portable",
                MessageBoxButtons.OK,
                MessageBoxIcon.Error);
        }
    }

    private static byte[] ReadResource(string name)
    {
        Stream stream = Assembly.GetExecutingAssembly().GetManifestResourceStream(name);
        if (stream == null)
        {
            throw new InvalidOperationException("Arquivo interno ausente: " + name);
        }
        using (stream)
        using (MemoryStream output = new MemoryStream())
        {
            stream.CopyTo(output);
            return output.ToArray();
        }
    }

    private static string ShortHash(byte[] bytes)
    {
        using (SHA256 sha = SHA256.Create())
        {
            byte[] hash = sha.ComputeHash(bytes);
            StringBuilder value = new StringBuilder(16);
            for (int index = 0; index < 8; index++)
            {
                value.Append(hash[index].ToString("x2"));
            }
            return value.ToString();
        }
    }

    private static void WriteIfChanged(string path, byte[] bytes)
    {
        if (File.Exists(path) && new FileInfo(path).Length == bytes.Length)
        {
            return;
        }
        string temporary = path + ".tmp";
        File.WriteAllBytes(temporary, bytes);
        if (File.Exists(path))
        {
            File.Delete(path);
        }
        File.Move(temporary, path);
    }

    private static string ForwardedArguments()
    {
        string[] args = Environment.GetCommandLineArgs();
        StringBuilder forwarded = new StringBuilder();
        for (int index = 1; index < args.Length; index++)
        {
            if (index > 1)
            {
                forwarded.Append(' ');
            }
            forwarded.Append('"');
            forwarded.Append(args[index].Replace("\\", "\\\\").Replace("\"", "\\\""));
            forwarded.Append('"');
        }
        return forwarded.ToString();
    }
}
