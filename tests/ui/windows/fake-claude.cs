// A stand-in for Claude Code that is installed but signed out, compiled with Add-Type (helpers.ps1)
// and copied by driver.ps1 to %APPDATA%\Claude\claude-code\<version>\claude.exe, where the Claude
// desktop app keeps its own copy. `auth status` answers signed out and `--version` answers a
// version; anything else, such as `auth login`, does nothing and fails, so the test can never sign
// in to anything. Every call is logged to calls.log next to the program.
using System;
using System.IO;

public static class FakeClaude {
  public static int Main(string[] args) {
    string joined = string.Join(" ", args);
    try {
      string dir = Path.GetDirectoryName(System.Reflection.Assembly.GetExecutingAssembly().Location);
      File.AppendAllText(Path.Combine(dir, "calls.log"), DateTime.Now.ToString("HH:mm:ss.fff") + " " + joined + Environment.NewLine);
    } catch { }
    if (joined == "--version") { Console.WriteLine("2.1.0 (Claude Code)"); return 0; }
    if (args.Length >= 2 && args[0] == "auth" && args[1] == "status") {
      Console.WriteLine("{\"loggedIn\": false, \"authMethod\": \"none\"}");
      return 1;
    }
    Console.Error.WriteLine("This is a stand-in for Claude Code in a test; it does nothing.");
    return 1;
  }
}
