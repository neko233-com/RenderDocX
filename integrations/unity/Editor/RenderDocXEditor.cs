using System;
using System.Diagnostics;
using UnityEditor;
using UnityEngine;

namespace RenderDocX.Editor
{
    internal static class RenderDocXEditor
    {
        private const string ExecutableKey = "RenderDocX.Executable";

        [MenuItem("Tools/RenderDocX/Set executable...")]
        private static void SetExecutable()
        {
            var path = EditorUtility.OpenFilePanel(EditorText.SelectExecutable, "", "");
            if (!string.IsNullOrEmpty(path)) EditorPrefs.SetString(ExecutableKey, path);
        }

        [MenuItem("Tools/RenderDocX/Open workspace")]
        private static void OpenWorkspace() => Start(null, null);

        [MenuItem("Tools/RenderDocX/Connect current Editor")]
        private static void ConnectCurrentEditor()
        {
            using (var process = Process.GetCurrentProcess()) Start(null, process.Id);
        }

        [MenuItem("Tools/RenderDocX/Connect Player by PID...")]
        private static void ConnectPlayer() => RenderDocXConnectWindow.Open();

        [MenuItem("Tools/RenderDocX/Open capture...")]
        private static void OpenCapture()
        {
            var path = EditorUtility.OpenFilePanel(EditorText.OpenCapture, "", "rdc");
            if (!string.IsNullOrEmpty(path)) Start(path, null);
        }

        internal static bool Start(string capture, int? pid)
        {
            var executable = EditorPrefs.GetString(ExecutableKey, "");
            if (string.IsNullOrEmpty(executable) || !System.IO.File.Exists(executable))
            {
                EditorUtility.DisplayDialog("RenderDocX", EditorText.ExecutableMissing, EditorText.Ok);
                return false;
            }

            try
            {
                Process.Start(CreateStartInfo(executable, capture, pid));
                return true;
            }
            catch (Exception error)
            {
                EditorUtility.DisplayDialog("RenderDocX", error.Message, EditorText.Ok);
                return false;
            }
        }

        internal static ProcessStartInfo CreateStartInfo(string executable, string capture, int? pid)
        {
            var info = new ProcessStartInfo(executable) { UseShellExecute = false };
            // Avoid shell parsing and preserve arbitrary Unicode and spaces in the path.
            if (!string.IsNullOrEmpty(capture)) info.EnvironmentVariables["RENDERDOCX_OPEN_CAPTURE"] = capture;
            else info.EnvironmentVariables.Remove("RENDERDOCX_OPEN_CAPTURE");
            if (pid.HasValue)
                info.EnvironmentVariables["RENDERDOCX_CONNECT_PID"] = pid.Value.ToString(System.Globalization.CultureInfo.InvariantCulture);
            else info.EnvironmentVariables.Remove("RENDERDOCX_CONNECT_PID");
            return info;
        }
    }

    internal static class EditorText
    {
        private static string Select(string en, string zh, string ja, string ko)
        {
            var language = Application.systemLanguage;
            if (language == SystemLanguage.Chinese || language == SystemLanguage.ChineseSimplified) return zh;
            if (language == SystemLanguage.Japanese) return ja;
            if (language == SystemLanguage.Korean) return ko;
            return en;
        }

        internal static string SelectExecutable => Select("Select RenderDocX executable", "选择 RenderDocX 可执行文件", "RenderDocX 実行ファイルを選択", "RenderDocX 실행 파일 선택");
        internal static string OpenCapture => Select("Open RenderDoc capture", "打开 RenderDoc 捕获", "RenderDoc キャプチャを開く", "RenderDoc 캡처 열기");
        internal static string ExecutableMissing => Select("Set the RenderDocX executable first in Tools > RenderDocX.", "请先在 Tools > RenderDocX 中设置 RenderDocX 可执行文件。", "先に Tools > RenderDocX で実行ファイルを設定してください。", "먼저 Tools > RenderDocX에서 실행 파일을 설정하세요.");
        internal static string Ok => Select("OK", "确定", "OK", "확인");
        internal static string Title => Select("Connect Player", "连接 Player", "Player に接続", "Player 연결");
        internal static string Instructions => Select("Enter the process ID of a local Player already running with RenderDoc. A busy client will not be disconnected.", "输入已加载 RenderDoc 的本地 Player 进程 ID。不会断开正在使用的客户端。", "RenderDoc を読み込んだローカル Player のプロセス ID を入力してください。使用中の接続は切断しません。", "RenderDoc이 로드된 로컬 Player의 프로세스 ID를 입력하세요. 사용 중인 연결은 끊지 않습니다.");
        internal static string ProcessId => Select("Process ID", "进程 ID", "プロセス ID", "프로세스 ID");
        internal static string Connect => Select("Connect", "连接", "接続", "연결");
        internal static string InvalidPid => Select("Enter a positive process ID.", "请输入大于零的进程 ID。", "正のプロセス ID を入力してください。", "0보다 큰 프로세스 ID를 입력하세요.");
    }

    internal sealed class RenderDocXConnectWindow : EditorWindow
    {
        private string processId = "";

        internal static void Open()
        {
            var window = GetWindow<RenderDocXConnectWindow>(true, EditorText.Title);
            window.minSize = new Vector2(360, 120);
            window.Show();
        }

        internal static bool TryParsePid(string value, out int pid)
        {
            return int.TryParse(value == null ? null : value.Trim(),
                System.Globalization.NumberStyles.None, System.Globalization.CultureInfo.InvariantCulture,
                out pid) && pid > 0;
        }

        private void OnGUI()
        {
            EditorGUILayout.HelpBox(EditorText.Instructions, MessageType.Info);
            processId = EditorGUILayout.TextField(EditorText.ProcessId, processId);
            int pid;
            var valid = TryParsePid(processId, out pid);
            if (!valid && !string.IsNullOrEmpty(processId))
                EditorGUILayout.HelpBox(EditorText.InvalidPid, MessageType.Warning);
            using (new EditorGUI.DisabledScope(!valid))
            {
                if (GUILayout.Button(EditorText.Connect))
                {
                    if (RenderDocXEditor.Start(null, pid)) Close();
                }
            }
        }
    }
}
