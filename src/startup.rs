use crate::{Settings, platform};
use std::path::{Path, PathBuf};

fn ps_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// No passwords, SYSTEM account, execution-policy changes or command-shell action.
pub fn script(exe: &Path, enable: bool, validate_only: bool) -> Result<String, String> {
    let exe = exe.to_str().ok_or("Путь приложения не является Unicode")?;
    let set = if enable {
        r#"
$action=New-ScheduledTaskAction -Execute $exe -Argument '--autorun' -WorkingDirectory ([IO.Path]::GetDirectoryName($exe))
$trigger=New-ScheduledTaskTrigger -AtLogOn -User $sid
$principal=New-ScheduledTaskPrincipal -UserId $sid -LogonType Interactive -RunLevel Highest
$settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -StartWhenAvailable
$task=New-ScheduledTask -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description 'Relay portable: restore saved connection settings at user logon'
if($validateOnly){if($task.Actions.Execute -ne $exe -or $task.Actions.Arguments -ne '--autorun'){throw 'Invalid startup action'};Write-Output 'OK';return}
Register-ScheduledTask -TaskName $taskName -InputObject $task -Force | Out-Null
$check=Get-ScheduledTask -TaskName $taskName
if($check.Actions.Execute -ne $exe){throw 'Startup verification failed'}
"#
    } else {
        r#"
if($validateOnly){Write-Output 'OK';return}
$task=Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if($null -ne $task){Unregister-ScheduledTask -TaskName $taskName -Confirm:$false}
"#
    };
    Ok(format!(
        r#"$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$exe={}
$validateOnly=${}
$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$taskName='RelayPortable-'+$sid
{}
"#,
        ps_string(exe),
        if validate_only { "true" } else { "false" },
        set
    ))
}

pub fn execute(exe: &Path, enable: bool, validate_only: bool) -> Result<(), String> {
    if !validate_only && !platform::is_admin() {
        return Err(
            "Для настройки автозапуска перезапустите Relay с правами администратора.".into(),
        );
    }
    let shell =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = std::process::Command::new(shell);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script(exe, enable, validate_only)?,
    ]);
    let output = platform::hidden(&mut command)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Не удалось изменить автозапуск: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

pub fn configure(settings: Settings, data: &Path) -> Result<Settings, String> {
    let path = data.join("config.json");
    let old = Settings::load(&path)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    execute(&exe, settings.auto_start, false)?;
    if let Err(e) = settings.save(&path) {
        let rollback = execute(&exe, old.auto_start, false);
        return Err(format!(
            "Настройки не сохранены: {e}. Откат автозапуска: {}",
            rollback.err().unwrap_or_else(|| "выполнен".into())
        ));
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn executable_paths_are_literal_even_with_apostrophes_and_dollars() {
        let s = script(
            Path::new("C:\\Test O'Brien\\$literal\\Relay.exe"),
            true,
            true,
        )
        .unwrap();
        assert!(s.contains("$exe='C:\\Test O''Brien\\$literal\\Relay.exe'"));
        assert!(s.contains("-LogonType Interactive -RunLevel Highest"));
        assert!(s.contains("-ExecutionTimeLimit ([TimeSpan]::Zero)"));
        assert!(!s.contains("-Password"));
        assert!(!s.contains("-ExecutionPolicy"));
    }
    #[test]
    fn disabling_addresses_only_our_user_scoped_task() {
        let s = script(Path::new("C:\\Relay.exe"), false, true).unwrap();
        assert!(s.contains("'RelayPortable-'+$sid"));
        assert!(!s.contains("Register-ScheduledTask"));
        assert!(s.contains("Unregister-ScheduledTask -TaskName $taskName"));
    }
    #[cfg(windows)]
    #[test]
    fn scheduler_objects_validate_without_registering_a_task() {
        execute(Path::new("C:\\Relay Test\\O'Brien\\Relay.exe"), true, true).unwrap();
    }
}
