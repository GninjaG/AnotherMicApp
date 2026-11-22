param([Parameter(Mandatory)][string]$Name)
# Define o dispositivo de reprodução padrão (console, multimídia e comunicação) por parte do nome. Não precisa de admin.
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
[ComImport, Guid("f8679f50-850a-41cf-9c72-430f290290c8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IPolicyConfig {
    int a(); int b(); int c(); int d(); int e(); int f(); int g(); int h(); int i(); int j();
    int SetDefaultEndpoint([MarshalAs(UnmanagedType.LPWStr)] string id, int role);
}
[ComImport, Guid("870af99c-171d-4f9e-af0d-e63df40c2bc9")] class PolicyConfigClient {}
public static class AudioDefault {
    public static int Set(string id) {
        var p = (IPolicyConfig)new PolicyConfigClient();
        int r = 0;
        for (int role = 0; role < 3; role++) r |= p.SetDefaultEndpoint(id, role);
        return r;
    }
}
"@
$root = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render"
$found = foreach ($k in Get-ChildItem $root) {
    $props = Get-ItemProperty "$($k.PSPath)\Properties" -ErrorAction SilentlyContinue
    $state = (Get-ItemProperty $k.PSPath).DeviceState
    $n = $props.'{a45c254e-df1c-4efd-8020-67d146a850e0},2'
    if ($state -eq 1 -and $n -like "*$Name*") { [pscustomobject]@{ Name = $n; Id = "{0.0.0.00000000}.$($k.PSChildName)" } }
}
if (@($found).Count -ne 1) { throw "esperava 1 dispositivo ativo com '$Name', achei: $(@($found).Name -join ', ')" }
$r = [AudioDefault]::Set($found.Id)
"padrão -> $($found.Name) (hr=$r)"
