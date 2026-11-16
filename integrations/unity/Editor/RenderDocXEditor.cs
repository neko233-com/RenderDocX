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
        private static void OpenWorkspace() => Start(null);

        [MenuItem("Tools/RenderDocX/Open capture...")]
        private static void OpenCapture()
        {
            var path = EditorUtility.OpenFilePanel("Open RenderDoc capture", "", "rdc");
            if (!string.IsNullOrEmpty(path)) Start(path);
        }

        private static void Start(string capture)
        {
            var executable = EditorPrefs.GetString(ExecutableKey, "");
            if (string.IsNullOrEmpty(executable) || !System.IO.File.Exists(executable))
            {
                EditorUtility.DisplayDialog("RenderDocX", "Set the RenderDocX executable first in Tools > RenderDocX.", "OK");
                return;
            }

            try
            {
                var info = new ProcessStartInfo(executable) { UseShellExecute = false };
                // Avoid shell parsing and preserve arbitrary Unicode and spaces in the path.
                if (!string.IsNullOrEmpty(capture)) info.EnvironmentVariables["RENDERDOCX_OPEN_CAPTURE"] = capture;
                else info.EnvironmentVariables.Remove("RENDERDOCX_OPEN_CAPTURE");
                Process.Start(info);
            }
            catch (Exception error)
            {
                EditorUtility.DisplayDialog("RenderDocX", error.Message, "OK");
            }
        }
    }
}
