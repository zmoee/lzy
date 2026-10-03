//! 右键菜单是 Windows 资源管理器的机制，其他平台没有。
//! Linux 上对应的是 Nautilus/Nemo 的 .desktop action，机制不同，没做。

pub fn install(_folder_id: i64) -> Result<String, String> {
    Err("右键菜单只支持 Windows（Linux 上对应的是文件管理器的 action，机制不同）".into())
}

pub fn uninstall() -> Result<bool, String> {
    Err("右键菜单只支持 Windows".into())
}
