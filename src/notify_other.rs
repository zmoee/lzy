//! 非 Windows 平台上 `--notify` 没有系统弹窗可弹，退化成往 stderr 说一句。
//! 这段被 cfg 挡在 Windows 编译之外，所以 Linux 版不会带上 syscall 那套。

pub fn show(title: &str, msg: &str, _is_err: bool) {
    note!("{title}: {msg}");
}
