using System;
using System.Diagnostics;
using System.IO;
using UnityEditor;
using UnityEngine;

// Keeps a disposable batch Editor alive for external live-target tests.
public static class RenderDocXLive
{
    private static string stopPath;
    private static double deadline;

    public static void Run()
    {
        var readyPath = Environment.GetEnvironmentVariable("RENDERDOCX_UNITY_READY");
        stopPath = Environment.GetEnvironmentVariable("RENDERDOCX_UNITY_STOP");
        if (string.IsNullOrEmpty(readyPath) || string.IsNullOrEmpty(stopPath))
        {
            UnityEngine.Debug.LogError("Missing live-test synchronization paths");
            EditorApplication.Exit(2);
            return;
        }
        using (var process = Process.GetCurrentProcess())
            File.WriteAllText(readyPath, process.Id + "\n" + SystemInfo.graphicsDeviceType);
        deadline = EditorApplication.timeSinceStartup + 120;
        EditorApplication.update += Poll;
    }

    private static void Poll()
    {
        if (!File.Exists(stopPath) && EditorApplication.timeSinceStartup < deadline) return;
        EditorApplication.update -= Poll;
        EditorApplication.Exit(File.Exists(stopPath) ? 0 : 1);
    }
}
