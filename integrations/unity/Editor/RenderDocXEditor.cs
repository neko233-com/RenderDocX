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

        [MenuItem("Tools/RenderDocX/Capture next Game frame")]
        private static void CaptureNextFrame()
        {
            try
            {
                var result = FrameCapture.Schedule(outcome =>
                {
                    UnityEngine.Debug.Log("RenderDocX: " + EditorText.CaptureResult(outcome));
                });
                if (result == FrameCaptureStart.Scheduled)
                    UnityEngine.Debug.Log("RenderDocX: " + EditorText.CaptureScheduled);
                else
                    EditorUtility.DisplayDialog("RenderDocX", EditorText.CaptureStartError(result), EditorText.Ok);
            }
            catch (Exception error)
            {
                UnityEngine.Debug.LogException(error);
                EditorUtility.DisplayDialog("RenderDocX", EditorText.CaptureFailed, EditorText.Ok);
            }
        }

        [MenuItem("Tools/RenderDocX/Cancel scheduled capture")]
        private static void CancelCapture()
        {
            if (!FrameCapture.Cancel())
                EditorUtility.DisplayDialog("RenderDocX", EditorText.NoScheduledCapture, EditorText.Ok);
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
        internal static string CaptureScheduled => Select("Capture scheduled for the next Game frame.", "已安排捕获下一游戏帧。", "次のゲームフレームのキャプチャを予約しました。", "다음 게임 프레임 캡처를 예약했습니다.");
        internal static string CaptureFailed => Select("Could not start or finish the capture scope. See the Console.", "无法开始或结束捕获范围，请查看控制台。", "キャプチャを開始または終了できません。コンソールを確認してください。", "캡처를 시작하거나 종료하지 못했습니다. 콘솔을 확인하세요.");
        internal static string NoScheduledCapture => Select("No capture is scheduled.", "没有待处理的捕获。", "予約されたキャプチャはありません。", "예약된 캡처가 없습니다.");
        internal static string CaptureStartError(FrameCaptureStart value)
        {
            switch (value)
            {
                case FrameCaptureStart.NotPlaying: return Select("Enter Play mode before capturing a Game frame.", "请先进入播放模式再捕获游戏帧。", "ゲームフレームをキャプチャする前に再生モードにしてください。", "게임 프레임을 캡처하려면 재생 모드로 들어가세요.");
                case FrameCaptureStart.BatchMode: return Select("Frame capture is unavailable in batch mode.", "批处理模式下无法捕获帧。", "バッチモードではフレームをキャプチャできません。", "배치 모드에서는 프레임을 캡처할 수 없습니다.");
                case FrameCaptureStart.Busy: return Select("A capture is already active or scheduled.", "已有捕获正在进行或等待执行。", "キャプチャが実行中または予約済みです。", "캡처가 이미 진행 중이거나 예약되어 있습니다.");
                default: return Select("Attach a supported external GPU profiler first.", "请先连接受支持的外部 GPU 分析器。", "先に対応する外部 GPU プロファイラーを接続してください。", "먼저 지원되는 외부 GPU 프로파일러를 연결하세요.");
            }
        }
        internal static string CaptureResult(FrameCaptureResult value)
        {
            switch (value)
            {
                case FrameCaptureResult.Ended: return Select("Capture scope ended; check the profiler for a saved capture.", "捕获范围已结束，请在分析器中确认捕获文件。", "キャプチャ範囲が終了しました。保存結果をプロファイラーで確認してください。", "캡처 범위가 종료되었습니다. 프로파일러에서 저장 결과를 확인하세요.");
                case FrameCaptureResult.ProfilerDetached: return Select("Profiler detached or another capture started before the frame.", "帧开始前分析器已断开或其他捕获已开始。", "フレーム前にプロファイラーが切断されたか、別のキャプチャが開始されました。", "프레임 전에 프로파일러가 분리되었거나 다른 캡처가 시작되었습니다.");
                case FrameCaptureResult.Cancelled: return Select("Scheduled capture cancelled.", "已取消待处理的捕获。", "予約されたキャプチャをキャンセルしました。", "예약된 캡처가 취소되었습니다.");
                default: return CaptureFailed;
            }
        }
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
