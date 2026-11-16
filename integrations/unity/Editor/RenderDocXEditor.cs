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
            var path = EditorUtility.OpenFilePanel("Select RenderDocX executable", "", "");
            if (!string.IsNullOrEmpty(path)) EditorPrefs.SetString(ExecutableKey, path);
        }

        [MenuItem("Tools/RenderDocX/Open workspace")]
        private static void OpenWorkspace() => Start(null, false);

        [MenuItem("Tools/RenderDocX/Connect current Editor")]
        private static void ConnectCurrentEditor() => Start(null, true);

        [MenuItem("Tools/RenderDocX/Open capture...")]
        private static void OpenCapture()
        {
            var path = EditorUtility.OpenFilePanel("Open RenderDoc capture", "", "rdc");
            if (!string.IsNullOrEmpty(path)) Start(path, false);
        }

        private static void Start(string capture, bool connectEditor)
        {
            var executable = EditorPrefs.GetString(ExecutableKey, "");
            if (string.IsNullOrEmpty(executable) || !System.IO.File.Exists(executable))
            {
                EditorUtility.DisplayDialog("RenderDocX", "Set the RenderDocX executable first in Tools > RenderDocX.", "OK");
                return;
            }

            try
            {
                int? pid = null;
                if (connectEditor)
                {
                    using (var process = Process.GetCurrentProcess())
                        pid = process.Id;
                }
                Process.Start(CreateStartInfo(executable, capture, pid));
            }
            catch (Exception error)
            {
                EditorUtility.DisplayDialog("RenderDocX", error.Message, "OK");
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
}
