//! Public macOS Accessibility API. No keyboard injection or permission bypass.
use std::{
    ffi::{c_char, c_void, CString},
    ptr,
};

type Raw = *const c_void;
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> Raw;
    fn AXUIElementCopyAttributeValue(element: Raw, attribute: Raw, value: *mut Raw) -> i32;
    fn AXUIElementPerformAction(element: Raw, action: Raw) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Raw, timeout: f32) -> i32;
    fn CGSessionCopyCurrentDictionary() -> Raw;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: Raw);
    fn CFRetain(value: Raw) -> Raw;
    fn CFStringCreateWithCString(allocator: Raw, text: *const c_char, encoding: u32) -> Raw;
    fn CFStringGetCString(value: Raw, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
    fn CFGetTypeID(value: Raw) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(value: Raw) -> isize;
    fn CFArrayGetValueAtIndex(value: Raw, index: isize) -> Raw;
    fn CFDictionaryGetValue(value: Raw, key: Raw) -> Raw;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Raw) -> bool;
}
const UTF8: u32 = 0x08000100;
struct Ref(Raw);
impl Drop for Ref {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}
impl Ref {
    fn string(value: &str) -> Self {
        let value = CString::new(value).expect("constant AX attribute");
        Self(unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), UTF8) })
    }
    fn attr(&self, name: &str) -> Option<Self> {
        let mut value = ptr::null();
        let result =
            unsafe { AXUIElementCopyAttributeValue(self.0, Self::string(name).0, &mut value) };
        (result == 0 && !value.is_null()).then_some(Self(value))
    }
    fn text(&self) -> String {
        if self.0.is_null() || unsafe { CFGetTypeID(self.0) != CFStringGetTypeID() } {
            return String::new();
        }
        let mut buffer = vec![0u8; 65536];
        if !unsafe {
            CFStringGetCString(
                self.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as isize,
                UTF8,
            )
        } {
            return String::new();
        }
        let end = buffer.iter().position(|v| *v == 0).unwrap_or(0);
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    }
    fn text_attr(&self, name: &str) -> String {
        self.attr(name).map(|v| v.text()).unwrap_or_default()
    }
    fn children(&self) -> Vec<Self> {
        let Some(array) = self.attr("AXChildren") else {
            return vec![];
        };
        if unsafe { CFGetTypeID(array.0) != CFArrayGetTypeID() } {
            return vec![];
        }
        (0..unsafe { CFArrayGetCount(array.0) }.min(6000))
            .map(|i| Self(unsafe { CFRetain(CFArrayGetValueAtIndex(array.0, i)) }))
            .collect()
    }
    fn enabled(&self) -> bool {
        self.attr("AXEnabled").is_some_and(|v| unsafe {
            CFGetTypeID(v.0) == CFBooleanGetTypeID() && CFBooleanGetValue(v.0)
        })
    }
}

pub fn ready() -> Result<(), String> {
    if !unsafe { AXIsProcessTrusted() } {
        return Err("请在电脑的系统设置 → 隐私与安全性 → 辅助功能中允许 ClawKit Desktop，然后重试；尚未发送".into());
    }
    let session = Ref(unsafe { CGSessionCopyCurrentDictionary() });
    if session.0.is_null() {
        return Err("电脑尚未登录，请解锁电脑后新建对话".into());
    }
    let locked =
        unsafe { CFDictionaryGetValue(session.0, Ref::string("CGSSessionScreenIsLocked").0) };
    let console =
        unsafe { CFDictionaryGetValue(session.0, Ref::string("kCGSSessionOnConsoleKey").0) };
    let boolean = |v: Raw| {
        !v.is_null() && unsafe { CFGetTypeID(v) == CFBooleanGetTypeID() && CFBooleanGetValue(v) }
    };
    if boolean(locked) || !boolean(console) {
        return Err("电脑已锁屏或不在当前登录会话，请解锁后新建对话".into());
    }
    Ok(())
}

pub struct Node {
    element: Ref,
    pub role: String,
    pub label: String,
    pub value: String,
    pub parent: Option<usize>,
}
pub struct Window {
    nodes: Vec<Node>,
}
impl Window {
    pub fn read(pid: i32) -> Result<Self, String> {
        ready()?;
        let app = Ref(unsafe { AXUIElementCreateApplication(pid) });
        unsafe {
            AXUIElementSetMessagingTimeout(app.0, 1.0);
        }
        let window = app
            .attr("AXFocusedWindow")
            .ok_or("找不到 Codex 当前窗口，请在电脑上打开 Codex")?;
        let mut nodes = vec![];
        let mut stack = vec![(window, None, 0)];
        while let Some((element, parent, depth)) = stack.pop() {
            if nodes.len() >= 6000 || depth > 60 {
                return Err("Codex 界面过于复杂，无法安全确认新建窗口".into());
            }
            let role = element.text_attr("AXRole");
            let label = ["AXTitle", "AXDescription", "AXHelp"]
                .iter()
                .map(|a| element.text_attr(a))
                .find(|s| !s.is_empty())
                .unwrap_or_default();
            let value = element.text_attr("AXValue");
            let index = nodes.len();
            for child in element.children().into_iter().rev() {
                stack.push((child, Some(index), depth + 1));
            }
            nodes.push(Node {
                element,
                role,
                label,
                value,
                parent,
            });
        }
        if !nodes.iter().any(|n| n.role == "AXWebArea") {
            return Err("Codex 界面尚未就绪，请在电脑打开窗口后重试".into());
        }
        Ok(Self { nodes })
    }

    pub fn has_draft(&self) -> bool {
        let send_enabled = self.nodes.iter().any(|n| {
            n.role == "AXButton"
                && ["发送", "Send"].contains(&n.label.as_str())
                && n.element.enabled()
        });
        self.nodes.iter().any(|n| {
            n.role == "AXTextArea"
                && !n.value.trim().is_empty()
                && (send_enabled
                    || ![
                        "随心输入",
                        "Ask anything",
                        "使用 ChatGPT Work",
                        "Ask ChatGPT Work",
                    ]
                    .contains(&n.value.trim()))
        })
    }

    fn descendants_text(&self, parent: usize) -> String {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                let mut p = Some(*i);
                while let Some(index) = p {
                    if index == parent {
                        return true;
                    }
                    p = self.nodes[index].parent;
                }
                false
            })
            .map(|(_, n)| format!("{} {}", n.label, n.value))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn send_index(&self, name: &str, text: &str) -> Result<usize, String> {
        let inputs: Vec<_> = self
            .nodes
            .iter()
            .filter(|n| n.role == "AXTextArea")
            .collect();
        let project = self.nodes.iter().any(|n| {
            n.role == "AXPopUpButton"
                && [
                    format!("切换项目：{name}"),
                    format!("Switch project: {name}"),
                ]
                .contains(&n.label)
        });
        let codex = self.nodes.iter().any(|n| {
            n.role == "AXPopUpButton"
                && [
                    "切换模式，当前模式：Codex",
                    "Switch mode, current mode: Codex",
                ]
                .contains(&n.label.as_str())
        });
        let local = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.role == "AXPopUpButton"
                    && ["选择聊天的运行位置", "Choose where the chat runs"]
                        .contains(&n.label.as_str())
            })
            .any(|(i, _)| {
                self.descendants_text(i)
                    .split_whitespace()
                    .any(|t| ["本地", "Local"].contains(&t))
            });
        let buttons: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.role == "AXButton"
                    && ["发送", "Send"].contains(&n.label.as_str())
                    && n.element.enabled()
            })
            .collect();
        if inputs.len() != 1
            || inputs[0].value.trim() != text.trim()
            || !project
            || !local
            || !codex
            || buttons.len() != 1
        {
            return Err("尚未确认 Codex 新建页的项目、内容和本地运行位置；尚未发送".into());
        }
        Ok(buttons[0].0)
    }
    pub fn validate(&self, name: &str, text: &str) -> Result<(), String> {
        self.send_index(name, text).map(|_| ())
    }
    pub fn press_send(&self, name: &str, text: &str) -> Result<(), String> {
        ready()?;
        let index = self.send_index(name, text)?;
        let result = unsafe {
            AXUIElementPerformAction(self.nodes[index].element.0, Ref::string("AXPress").0)
        };
        if result == 0 {
            Ok(())
        } else {
            Err("未收到 Codex 发送按钮回执，请核对新建结果，不会自动重发".into())
        }
    }
}
