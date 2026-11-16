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
            Require(editorType.GetMethod("ConnectPlayer", BindingFlags.Static | BindingFlags.NonPublic) != null, "Player connection menu");
            Type windowType = editorType.Assembly.GetType("RenderDocX.Editor.RenderDocXConnectWindow");
            Require(windowType != null, "Player connection window");
            var parse = windowType.GetMethod("TryParsePid", BindingFlags.Static | BindingFlags.NonPublic);
            Require(parse != null, "PID parser");
            var valid = new object[] { " 12345 ", 0 };
            Require((bool)parse.Invoke(null, valid) && (int)valid[1] == 12345, "Valid Player PID");
            foreach (var bad in new[] { "", "0", "-1", "+1", "1 2", "2147483648" })
                Require(!(bool)parse.Invoke(null, new object[] { bad, 0 }), "Invalid Player PID: " + bad);
            var player = (ProcessStartInfo)method.Invoke(null, new object[] { "renderdocx.exe", null, 12345 });
            Require(player.EnvironmentVariables["RENDERDOCX_CONNECT_PID"] == "12345" && player.EnvironmentVariables["RENDERDOCX_OPEN_CAPTURE"] == null, "Player PID handoff");
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
