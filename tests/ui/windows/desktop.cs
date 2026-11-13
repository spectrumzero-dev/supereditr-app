// The Windows smoke tests' desktop helpers, compiled with Add-Type (helpers.ps1): real key presses
// through SendInput, windows by process and title, bringing a window to the front, screenshots, and
// JavaScript in a browser page through its DevTools port.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class Desk {
  [StructLayout(LayoutKind.Sequential)]
  struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Sequential)]
  struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit)]
  struct InputUnion { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
  [StructLayout(LayoutKind.Sequential)]
  struct INPUT { public uint type; public InputUnion u; }
  public struct RECT { public int Left, Top, Right, Bottom; }

  [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, INPUT[] inputs, int size);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);

  const uint KEYUP = 2, UNICODE = 4;

  static INPUT Key(ushort vk, bool up) {
    var i = new INPUT { type = 1 };
    i.u.ki = new KEYBDINPUT { wVk = vk, dwFlags = up ? KEYUP : 0 };
    // Arrows, Home, End, Insert, Delete and friends are extended keys.
    if ((vk >= 0x21 && vk <= 0x2E) || vk == 0xA3 || vk == 0xA5) i.u.ki.dwFlags |= 1;
    return i;
  }

  static void Send(List<INPUT> inputs) {
    if (inputs.Count == 0) return;
    SendInput((uint)inputs.Count, inputs.ToArray(), Marshal.SizeOf(typeof(INPUT)));
  }

  static readonly Dictionary<string, ushort> Names = new Dictionary<string, ushort>(StringComparer.OrdinalIgnoreCase) {
    {"ctrl", 0x11}, {"shift", 0x10}, {"alt", 0x12}, {"win", 0x5B},
    {"enter", 0x0D}, {"esc", 0x1B}, {"tab", 0x09}, {"space", 0x20}, {"backspace", 0x08},
    {"home", 0x24}, {"end", 0x23}, {"left", 0x25}, {"up", 0x26}, {"right", 0x27}, {"down", 0x28},
    {"insert", 0x2D}, {"delete", 0x2E}, {"slash", 0xBF},
  };

  static ushort Vk(string name) {
    ushort vk;
    if (Names.TryGetValue(name, out vk)) return vk;
    if (name.Length == 1 && char.IsLetterOrDigit(name[0])) return (ushort)char.ToUpperInvariant(name[0]);
    if (name.Length >= 2 && (name[0] == 'f' || name[0] == 'F')) {
      int n;
      if (int.TryParse(name.Substring(1), out n) && n >= 1 && n <= 24) return (ushort)(0x6F + n);
    }
    throw new ArgumentException("unknown key " + name);
  }

  /// <summary>Releases every modifier, so a key left down by an earlier step doesn't turn later keys into chords.</summary>
  public static void ReleaseModifiers() {
    var list = new List<INPUT>();
    foreach (ushort vk in new ushort[] { 0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5 }) list.Add(Key(vk, true));
    Send(list);
  }

  /// <summary>Presses one chord, such as "ctrl+shift+f2", "alt+enter" or "tab": modifiers down, the key, all up.</summary>
  public static void Press(string chord) {
    var parts = chord.Split('+');
    var down = new List<INPUT>();
    var up = new List<INPUT>();
    foreach (var p in parts) { var vk = Vk(p); down.Add(Key(vk, false)); up.Insert(0, Key(vk, true)); }
    // Down and up as separate batches, a moment apart, as a person's press is.
    Send(down);
    Thread.Sleep(30);
    Send(up);
    Thread.Sleep(30);
  }

  /// <summary>Types text as Unicode characters, independent of the keyboard layout.</summary>
  public static void Type(string text) {
    foreach (char c in text) {
      var list = new List<INPUT>();
      foreach (bool up in new[] { false, true }) {
        var i = new INPUT { type = 1 };
        i.u.ki = new KEYBDINPUT { wScan = c, dwFlags = UNICODE | (up ? KEYUP : 0) };
        list.Add(i);
      }
      Send(list);
      Thread.Sleep(15);
    }
  }

  public static uint Pid(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }
  public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
  public static string Class(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }

  static string ProcessName(uint pid) {
    try { return Process.GetProcessById((int)pid).ProcessName; } catch { return ""; }
  }

  /// <summary>Visible top-level windows of processes with this name (no ".exe"), whose title starts with titlePrefix.</summary>
  public static IntPtr[] Windows(string process, string titlePrefix) {
    var found = new List<IntPtr>();
    var names = new Dictionary<uint, string>();
    EnumWindows((h, l) => {
      if (!IsWindowVisible(h)) return true;
      uint pid = Pid(h);
      string name;
      if (!names.TryGetValue(pid, out name)) { name = ProcessName(pid); names[pid] = name; }
      if (string.Equals(name, process, StringComparison.OrdinalIgnoreCase) && Title(h).StartsWith(titlePrefix ?? "", StringComparison.Ordinal)) found.Add(h);
      return true;
    }, IntPtr.Zero);
    return found.ToArray();
  }

  /// <summary>Visible top-level windows of a process whose title is exactly title.</summary>
  public static IntPtr Exact(string process, string title) {
    foreach (var h in Windows(process, title)) if (Title(h) == title) return h;
    return IntPtr.Zero;
  }

  /// <summary>
  /// Brings a window to the front and reports whether it got there. Windows refuses
  /// SetForegroundWindow to a process that didn't get the last input, so a harmless key (a lone
  /// Shift) goes first, and the input queues are joined for the call as a fallback.
  /// </summary>
  public static bool Front(IntPtr h) {
    if (h == IntPtr.Zero) return false;
    for (int attempt = 0; attempt < 4; attempt++) {
      if (IsIconic(h)) ShowWindow(h, 9);
      Press("shift");
      SetForegroundWindow(h);
      Thread.Sleep(150);
      if (GetForegroundWindow() == h) return true;
      IntPtr fg = GetForegroundWindow();
      uint fgPid;
      uint fgThread = GetWindowThreadProcessId(fg, out fgPid);
      uint me = GetCurrentThreadId();
      AttachThreadInput(me, fgThread, true);
      BringWindowToTop(h);
      SetForegroundWindow(h);
      AttachThreadInput(me, fgThread, false);
      Thread.Sleep(250);
      if (GetForegroundWindow() == h) return true;
    }
    return false;
  }

  public static void Close(IntPtr h) { PostMessage(h, 0x0010, IntPtr.Zero, IntPtr.Zero); }

  /// <summary>
  /// Puts text on the clipboard from any thread, as another app's copy does: on a thread of its own
  /// in a single-threaded apartment, which the clipboard's OLE calls need, trying again while another
  /// program holds the clipboard open.
  /// </summary>
  public static bool Copy(string text) {
    bool copied = false;
    var t = new Thread(() => {
      try { System.Windows.Forms.Clipboard.SetDataObject(text, true, 100, 5); copied = true; } catch { }
    });
    t.SetApartmentState(ApartmentState.STA);
    t.Start();
    t.Join();
    return copied;
  }

  /// <summary>Brings a window to the front without any key press, so no modifier is touched.</summary>
  public static bool Steal(IntPtr h) {
    IntPtr fg = GetForegroundWindow();
    if (fg == h) return true;
    uint fgPid;
    uint fgThread = GetWindowThreadProcessId(fg, out fgPid);
    uint me = GetCurrentThreadId();
    AttachThreadInput(me, fgThread, true);
    BringWindowToTop(h);
    bool ok = SetForegroundWindow(h);
    AttachThreadInput(me, fgThread, false);
    return ok;
  }

  static string raceReport = "";

  /// <summary>What the last race did, once it has finished: when the panel hid and what happened then.</summary>
  public static string RaceReport() { lock (Names) return raceReport; }

  /// <summary>
  /// Acts while SuperEditr applies a fix: on a thread of its own, waits for the panel to hide (the
  /// apply key closes it), then after delayMs either brings window to the front (and, with holdMs,
  /// keeps taking it back every 5 ms for that long) or, with copy set, puts copy on the clipboard as
  /// the user copying something. Returns at once; RaceReport says what happened.
  /// </summary>
  public static void RaceAfterPanelHides(IntPtr panel, int delayMs, IntPtr window, int holdMs, string copy) {
    lock (Names) raceReport = "";
    var t = new Thread(() => {
      var sw = Stopwatch.StartNew();
      while (IsWindowVisible(panel) && sw.ElapsedMilliseconds < 20000) Thread.Sleep(1);
      long hidden = sw.ElapsedMilliseconds;
      if (delayMs > 0) Thread.Sleep(delayMs);
      string did;
      if (copy != null) {
        did = Copy(copy) ? "copied" : "couldn't copy";
      } else {
        int took = 0;
        var hold = Stopwatch.StartNew();
        do {
          if (GetForegroundWindow() != window) { Steal(window); took++; }
          if (holdMs > 0) Thread.Sleep(5);
        } while (hold.ElapsedMilliseconds < holdMs);
        did = string.Format("took the front {0} times", took);
      }
      lock (Names) raceReport = string.Format("panel hid after {0} ms; {1} {2} ms later", hidden, did, delayMs);
    });
    t.IsBackground = true;
    t.Start();
  }

  public static string Rect(IntPtr h) {
    RECT r;
    if (!GetWindowRect(h, out r)) return "";
    return string.Format("{0}x{1} at {2},{3}", r.Right - r.Left, r.Bottom - r.Top, r.Left, r.Top);
  }

  /// <summary>A screenshot of the whole desktop, in physical pixels.</summary>
  public static void Shot(string path) {
    var b = System.Windows.Forms.SystemInformation.VirtualScreen;
    using (var bmp = new Bitmap(b.Width, b.Height))
    using (var g = Graphics.FromImage(bmp)) {
      g.CopyFromScreen(b.Left, b.Top, 0, 0, bmp.Size);
      bmp.Save(path, ImageFormat.Png);
    }
  }

  static string JsonString(string s) {
    var sb = new StringBuilder("\"");
    foreach (char c in s) {
      switch (c) {
        case '"': sb.Append("\\\""); break;
        case '\\': sb.Append("\\\\"); break;
        case '\n': sb.Append("\\n"); break;
        case '\r': sb.Append("\\r"); break;
        case '\t': sb.Append("\\t"); break;
        default:
          if (c < 0x20) sb.AppendFormat("\\u{0:x4}", (int)c); else sb.Append(c);
          break;
      }
    }
    return sb.Append('"').ToString();
  }

  /// <summary>
  /// Evaluates a JavaScript expression in a page through the browser's DevTools WebSocket and
  /// returns the protocol's whole JSON answer, for the caller to parse. A small WebSocket client of
  /// its own, so the handshake sends only what the DevTools server accepts, and a refusal says why.
  /// </summary>
  public static string Evaluate(string webSocketUrl, string expression, int timeoutMs) {
    var uri = new Uri(webSocketUrl);
    using (var tcp = new TcpClient()) {
      tcp.ReceiveTimeout = timeoutMs;
      tcp.SendTimeout = timeoutMs;
      tcp.Connect("127.0.0.1", uri.Port);
      var stream = tcp.GetStream();
      string key = Convert.ToBase64String(Guid.NewGuid().ToByteArray());
      string request = "GET " + uri.PathAndQuery + " HTTP/1.1\r\nHost: 127.0.0.1:" + uri.Port +
        "\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: " + key + "\r\nSec-WebSocket-Version: 13\r\n\r\n";
      var bytes = Encoding.ASCII.GetBytes(request);
      stream.Write(bytes, 0, bytes.Length);
      // The response's head, byte by byte up to the blank line, so no frame is read with it.
      var head = new StringBuilder();
      while (!head.ToString().EndsWith("\r\n\r\n")) {
        int b = stream.ReadByte();
        if (b < 0) throw new Exception("the DevTools server closed the connection: " + head);
        head.Append((char)b);
      }
      if (!head.ToString().StartsWith("HTTP/1.1 101")) {
        var body = new byte[2048];
        int n = 0;
        try { n = stream.Read(body, 0, body.Length); } catch { }
        throw new Exception(head.ToString().Split('\r')[0] + ": " + Encoding.UTF8.GetString(body, 0, n));
      }
      string message = "{\"id\":1,\"method\":\"Runtime.evaluate\",\"params\":{\"expression\":" + JsonString(expression) +
        ",\"returnByValue\":true,\"awaitPromise\":true}}";
      SendFrame(stream, Encoding.UTF8.GetBytes(message));
      var answer = new List<byte>();
      while (true) {
        int b0 = ReadByte(stream), b1 = ReadByte(stream);
        long length = b1 & 0x7F;
        if (length == 126) length = (ReadByte(stream) << 8) | ReadByte(stream);
        else if (length == 127) { length = 0; for (int i = 0; i < 8; i++) length = (length << 8) | (uint)ReadByte(stream); }
        var payload = new byte[length];
        for (long read = 0; read < length;) read += stream.Read(payload, (int)read, (int)(length - read));
        int opcode = b0 & 0x0F;
        if (opcode == 8) throw new Exception("the DevTools server closed the WebSocket");
        if (opcode != 0 && opcode != 1) continue;   // pings and binary frames
        answer.AddRange(payload);
        if ((b0 & 0x80) == 0) continue;            // more of this message follows
        string text = Encoding.UTF8.GetString(answer.ToArray());
        answer.Clear();
        if (text.StartsWith("{\"id\":1,")) return text;
      }
    }
  }

  static int ReadByte(NetworkStream stream) {
    int b = stream.ReadByte();
    if (b < 0) throw new Exception("the DevTools server closed the connection");
    return b;
  }

  // One masked text frame, as a client must send.
  static void SendFrame(NetworkStream stream, byte[] payload) {
    var frame = new List<byte> { 0x81 };
    if (payload.Length < 126) frame.Add((byte)(0x80 | payload.Length));
    else if (payload.Length < 65536) { frame.Add(0x80 | 126); frame.Add((byte)(payload.Length >> 8)); frame.Add((byte)payload.Length); }
    else { frame.Add(0x80 | 127); for (int i = 7; i >= 0; i--) frame.Add((byte)((long)payload.Length >> (8 * i))); }
    var mask = Guid.NewGuid().ToByteArray();
    for (int i = 0; i < 4; i++) frame.Add(mask[i]);
    for (int i = 0; i < payload.Length; i++) frame.Add((byte)(payload[i] ^ mask[i % 4]));
    var bytes = frame.ToArray();
    stream.Write(bytes, 0, bytes.Length);
  }
}
