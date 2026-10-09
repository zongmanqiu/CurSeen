//! 版本号解析与比较。对齐 Python `update/checker.py:VersionChecker._parse`。

/// 三段式版本号。
///
/// Python 只取 `tag` 的前三段、非数字段整段作废（`(0,0,0)`）。这里保留同样的宽松度：
/// tag 里常见 `v1.2.3`、`1.2.3`、`1.2.3-beta`，都要能比。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const ZERO: Self = Self {
        major: 0,
        minor: 0,
        patch: 0,
    };

    /// 解析 `"v1.2.3"` / `"1.2"` / `"1.2.3-rc1"`。无法解析 → `None`。
    ///
    /// 缺省段补 0（`"1.2"` → 1.2.0）；`-` 之后的前后缀（`-beta`、`+build`）直接丢弃。
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        // 去掉预发布/构建元数据：1.2.3-beta.1 → 1.2.3
        let core = s.split(['-', '+']).next().unwrap_or(s);
        let mut it = core.split('.');
        let mut nums = [0u32; 3];
        let mut seen = 0usize;
        for slot in nums.iter_mut() {
            match it.next() {
                Some(part) => {
                    let t = part.trim();
                    if t.is_empty() {
                        // "1." 这种：缺段按 0 处理，但已读到的段数不再增加
                        break;
                    }
                    *slot = t.parse().ok()?;
                    seen += 1;
                }
                None => break,
            }
        }
        if seen == 0 {
            return None;
        }
        Some(Self {
            major: nums[0],
            minor: nums[1],
            patch: nums[2],
        })
    }

    /// 编译期植入的当前版本（`Cargo.toml` 的 `version`）。
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self::ZERO)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_prefixed() {
        assert_eq!(
            Version::parse("1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            Version::parse("v1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            Version::parse("V2.0.0"),
            Some(Version {
                major: 2,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse(" 1.2.3 "),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
    }

    #[test]
    fn missing_parts_default_to_zero() {
        assert_eq!(
            Version::parse("1"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("1.2"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 0
            })
        );
    }

    #[test]
    fn prerelease_suffix_is_dropped() {
        assert_eq!(
            Version::parse("1.2.3-beta.1"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(
            Version::parse("1.2.3+build7"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("abc"), None);
        assert_eq!(Version::parse("v"), None);
    }

    #[test]
    fn ordering_is_numeric_not_lexicographic() {
        // 字符串比较会把 "1.10.0" 判小于 "1.9.0"——版本号绝不能那样比。
        assert!(Version::parse("1.10.0") > Version::parse("1.9.0"));
        assert!(Version::parse("2.0.0") > Version::parse("1.99.99"));
        assert!(Version::parse("1.0.0") == Version::parse("v1.0.0"));
    }

    #[test]
    fn current_version_is_parsable() {
        assert!(Version::current() > Version::ZERO);
    }
}
