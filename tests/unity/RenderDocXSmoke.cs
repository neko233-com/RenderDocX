using System;
using System.Diagnostics;
using System.Reflection;
using UnityEditor;
using UnityEngine;

// Invoked by the Unity Editor against the installed local UPM package.
public static class RenderDocXSmoke
{
    public static void Check()
    {
        try
        {
            Type editorType = null;
            foreach (var assembly in AppDomain.CurrentDomain.GetAssemblies())
                editorType = editorType ?? assembly.GetType("RenderDocX.Editor.RenderDocXEditor");
            Require(editorType != null, "Editor integration is not compiled");
            var method = editorType.GetMethod("CreateStartInfo", BindingFlags.Static | BindingFlags.NonPublic);
            Require(method != null, "Missing handoff builder");
            var pid = Process.GetCurrentProcess().Id;
            var connect = (ProcessStartInfo)method.Invoke(null, new object[] { "renderdocx.exe", null, pid });
            Require(!connect.UseShellExecute && connect.EnvironmentVariables["RENDERDOCX_CONNECT_PID"] == pid.ToString(), "Editor PID handoff");
            Require(connect.EnvironmentVariables["RENDERDOCX_OPEN_CAPTURE"] == null, "Connection clears capture");
            const string capture = "D:/captures/测试 한글 日本語.rdc";
            var open = (ProcessStartInfo)method.Invoke(null, new object[] { "renderdocx.exe", capture, null });
            Require(open.EnvironmentVariables["RENDERDOCX_OPEN_CAPTURE"] == capture, "Unicode capture path");
            Require(open.EnvironmentVariables["RENDERDOCX_CONNECT_PID"] == null, "Capture clears PID");
            var workspace = (ProcessStartInfo)method.Invoke(null, new object[] { "renderdocx.exe", null, null });
            Require(workspace.EnvironmentVariables["RENDERDOCX_OPEN_CAPTURE"] == null && workspace.EnvironmentVariables["RENDERDOCX_CONNECT_PID"] == null, "Workspace clears handoff");
            Require(editorType.GetMethod("ConnectCurrentEditor", BindingFlags.Static | BindingFlags.NonPublic) != null, "Connection menu");
            UnityEngine.Debug.Log("RENDERDOCX_UNITY_SMOKE_PASSED");
        }
        catch (Exception error)
        {
            UnityEngine.Debug.LogError("RENDERDOCX_UNITY_SMOKE_FAILED: " + error);
            EditorApplication.Exit(1);
        }
    }

    private static void Require(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }
}
