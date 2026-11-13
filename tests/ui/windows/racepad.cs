// A small text editor for the Windows smoke tests' race cases, built with Add-Type (helpers.ps1) into
// racepad.exe: one window titled "RacePad NAME" holding a plain Win32 edit control, which UI
// Automation reads and selects in like Notepad's. It can behave the ways a real app sometimes does:
//
//   racepad.exe NAME TEXTFILE [--paste-delay MS] [--hang-file FILE]
//
//   --paste-delay MS  Ctrl+V pastes MS milliseconds late, reading the clipboard only then, as a busy
//                     app does with a paste it has queued
//   --hang-file FILE  when FILE appears, the window stops responding for the number of milliseconds
//                     it holds (it is read and deleted first), as an app busy on its UI thread does
//
// The edit control starts with TEXTFILE's text. Every change is written to TEXTFILE.out, so the
// driver can read what the window holds without asking it, even while it doesn't respond.
// Keep this file C# 5: Windows PowerShell's Add-Type compiles it.
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class RacePad {
  delegate IntPtr WndProc(IntPtr h, uint m, IntPtr w, IntPtr l);

  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  struct WNDCLASS {
    public uint style; public WndProc proc; public int cls; public int wnd; public IntPtr instance;
    public IntPtr icon; public IntPtr cursor; public IntPtr background; public string menu; public string name;
  }
  [StructLayout(LayoutKind.Sequential)]
  struct MSG { public IntPtr h; public uint m; public IntPtr w; public IntPtr l; public uint time; public int x; public int y; }

  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern ushort RegisterClassW(ref WNDCLASS c);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr CreateWindowExW(uint ex, string cls, string title, uint style, int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr param);
  [DllImport("user32.dll")] static extern IntPtr DefWindowProcW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern IntPtr CallWindowProcW(IntPtr prev, IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern IntPtr SetWindowLongPtrW(IntPtr h, int index, WndProc proc);
  [DllImport("user32.dll")] static extern int GetMessageW(out MSG msg, IntPtr h, uint min, uint max);
  [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG msg);
  [DllImport("user32.dll")] static extern IntPtr DispatchMessageW(ref MSG msg);
  [DllImport("user32.dll")] static extern void PostQuitMessage(int code);
  [DllImport("user32.dll")] static extern IntPtr SetTimer(IntPtr h, IntPtr id, uint ms, IntPtr proc);
  [DllImport("user32.dll")] static extern bool KillTimer(IntPtr h, IntPtr id);
  [DllImport("user32.dll")] static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool repaint);
  [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern IntPtr SetFocus(IntPtr h);
  [DllImport("user32.dll")] static extern short GetKeyState(int vk);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern int GetWindowTextLengthW(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool SetWindowTextW(IntPtr h, string s);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("gdi32.dll", CharSet = CharSet.Unicode)] static extern IntPtr CreateFontW(int h, int w, int e, int o, int weight, uint i, uint u, uint s, uint cs, uint op, uint cp, uint q, uint pf, string face);
  [DllImport("kernel32.dll")] static extern IntPtr GetModuleHandleW(IntPtr name);
  [StructLayout(LayoutKind.Sequential)] struct RECT { public int Left, Top, Right, Bottom; }

  const uint WM_DESTROY = 0x2, WM_SIZE = 0x5, WM_SETFOCUS = 0x7, WM_SETFONT = 0x30, WM_COMMAND = 0x111;
  const uint WM_TIMER = 0x113, WM_KEYDOWN = 0x100, WM_CHAR = 0x102, WM_PASTE = 0x302, EM_SETSEL = 0xB1;
  const uint WS_OVERLAPPEDWINDOW = 0x00CF0000, WS_CHILD = 0x40000000, WS_VISIBLE = 0x10000000, WS_VSCROLL = 0x00200000;
  const uint ES_MULTILINE = 0x4, ES_AUTOVSCROLL = 0x40, ES_NOHIDESEL = 0x100;
  const int GWLP_WNDPROC = -4;
  static readonly IntPtr PASTE_TIMER = (IntPtr)1, HANG_TIMER = (IntPtr)2;

  static WndProc mainProc, editProc;
  static IntPtr main, edit, previousEditProc;
  static string output, hangFile;
  static int pasteDelay;

  static void Save() {
    var s = new StringBuilder(GetWindowTextLengthW(edit) + 1);
    GetWindowTextW(edit, s, s.Capacity);
    try { File.WriteAllText(output, s.ToString(), new UTF8Encoding(false)); } catch { }
  }

  static void Hang() {
    if (hangFile == null || !File.Exists(hangFile)) return;
    int ms = 0;
    try { int.TryParse(File.ReadAllText(hangFile).Trim(), out ms); File.Delete(hangFile); } catch { return; }
    // The UI thread stops: no messages, no accessibility answers, no keys.
    Thread.Sleep(ms);
  }

  static IntPtr MainProc(IntPtr h, uint m, IntPtr w, IntPtr l) {
    switch (m) {
      case WM_SIZE:
        RECT r;
        GetClientRect(h, out r);
        MoveWindow(edit, 0, 0, r.Right, r.Bottom, true);
        return IntPtr.Zero;
      case WM_SETFOCUS:
        SetFocus(edit);
        return IntPtr.Zero;
      case WM_COMMAND:
        // EN_CHANGE
        if (((long)w >> 16 & 0xFFFF) == 0x300) Save();
        return IntPtr.Zero;
      case WM_TIMER:
        if (w == PASTE_TIMER) { KillTimer(h, PASTE_TIMER); SendMessageW(edit, WM_PASTE, IntPtr.Zero, IntPtr.Zero); }
        if (w == HANG_TIMER) Hang();
        return IntPtr.Zero;
      case WM_DESTROY:
        PostQuitMessage(0);
        return IntPtr.Zero;
    }
    return DefWindowProcW(h, m, w, l);
  }

  // The edit control's keys: Ctrl+A selects everything, which a plain edit control doesn't do by
  // itself, and with a paste delay Ctrl+V only starts the timer.
  static IntPtr EditProc(IntPtr h, uint m, IntPtr w, IntPtr l) {
    bool ctrl = GetKeyState(0x11) < 0;
    if ((m == WM_KEYDOWN && (long)w == 'A' && ctrl) || (m == WM_CHAR && (long)w == 0x01)) {
      if (m == WM_KEYDOWN) SendMessageW(edit, EM_SETSEL, IntPtr.Zero, (IntPtr)(-1));
      return IntPtr.Zero;
    }
    if (pasteDelay > 0 && ((m == WM_KEYDOWN && (long)w == 'V' && ctrl) || (m == WM_CHAR && (long)w == 0x16))) {
      if (m == WM_KEYDOWN) SetTimer(main, PASTE_TIMER, (uint)pasteDelay, IntPtr.Zero);
      return IntPtr.Zero;
    }
    return CallWindowProcW(previousEditProc, h, m, w, l);
  }

  [STAThread]
  public static void Main(string[] args) {
    for (int i = 2; i + 1 < args.Length; i += 2) {
      if (args[i] == "--paste-delay") int.TryParse(args[i + 1], out pasteDelay);
      if (args[i] == "--hang-file") hangFile = args[i + 1];
    }
    output = args[1] + ".out";
    IntPtr instance = GetModuleHandleW(IntPtr.Zero);
    mainProc = MainProc;
    editProc = EditProc;
    var c = new WNDCLASS { proc = mainProc, instance = instance, name = "RacePad", background = (IntPtr)6 };
    RegisterClassW(ref c);
    main = CreateWindowExW(0, "RacePad", "RacePad " + args[0], WS_OVERLAPPEDWINDOW, 120, 160, 900, 300, IntPtr.Zero, IntPtr.Zero, instance, IntPtr.Zero);
    edit = CreateWindowExW(0, "EDIT", "", WS_CHILD | WS_VISIBLE | WS_VSCROLL | ES_MULTILINE | ES_AUTOVSCROLL | ES_NOHIDESEL, 0, 0, 900, 300, main, IntPtr.Zero, instance, IntPtr.Zero);
    SendMessageW(edit, WM_SETFONT, CreateFontW(-24, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0, "Consolas"), (IntPtr)1);
    SetWindowTextW(edit, File.ReadAllText(args[1], Encoding.UTF8));
    previousEditProc = SetWindowLongPtrW(edit, GWLP_WNDPROC, editProc);
    if (hangFile != null) SetTimer(main, HANG_TIMER, 50, IntPtr.Zero);
    Save();
    ShowWindow(main, 5);
    MSG msg;
    while (GetMessageW(out msg, IntPtr.Zero, 0, 0) > 0) { TranslateMessage(ref msg); DispatchMessageW(ref msg); }
  }
}
