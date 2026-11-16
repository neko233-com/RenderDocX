using System;
using UnityEditor;
using UnityEditor.PackageManager;
using UnityEditor.PackageManager.Requests;
using UnityEngine;

// Execute in an isolated test project without -quit: Package Manager is asynchronous.
public static class RenderDocXPackageInstall
{
    private static AddRequest request;
    private static double deadline;

    public static void Install()
    {
        var path = Environment.GetEnvironmentVariable("RENDERDOCX_TEST_PACKAGE");
        if (string.IsNullOrEmpty(path))
        {
            Debug.LogError("Missing RENDERDOCX_TEST_PACKAGE");
            EditorApplication.Exit(2);
            return;
        }
        request = Client.Add("file:" + path.Replace('\\', '/'));
        deadline = EditorApplication.timeSinceStartup + 180;
        EditorApplication.update += Poll;
    }

    private static void Poll()
    {
        if (!request.IsCompleted && EditorApplication.timeSinceStartup <= deadline) return;
        EditorApplication.update -= Poll;
        if (!request.IsCompleted || request.Status != StatusCode.Success)
        {
            Debug.LogError("RenderDocX package installation failed: " + (request.Error == null ? "timeout" : request.Error.message));
            EditorApplication.Exit(1);
            return;
        }
        Debug.Log("RENDERDOCX_PACKAGE_INSTALLED " + request.Result.name);
        EditorApplication.Exit(0);
    }
}
