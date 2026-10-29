use super::*;

/// Every font file under the host's system font directory.
fn host_fonts() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
            {
                out.push(path);
            }
        }
    }
    let mut fonts = Vec::new();
    walk(Path::new("/usr/share/fonts"), &mut fonts);
    fonts
}

/// The header-only `name` reader must agree with a full ttf-parser parse.
#[test]
fn face_families_match_full_parse() {
    let fonts = host_fonts();
    if fonts.is_empty() {
        eprintln!("no host fonts under /usr/share/fonts; skipping");
        return;
    }

    let mut checked = 0;
    for path in fonts {
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        let face_count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
        let expected: Vec<(u32, Option<String>)> = (0..face_count)
            .filter_map(|index| {
                let face = Face::parse(&data, index).ok()?;
                Some((index, family_name(face.names())))
            })
            .collect();
        if expected.is_empty() {
            continue;
        }

        let actual = face_families(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let actual: Vec<_> = actual
            .into_iter()
            .filter(|(index, _)| expected.iter().any(|(e, _)| e == index))
            .collect();
        assert_eq!(actual, expected, "{}", path.display());
        checked += 1;
    }
    eprintln!("checked {checked} font files");
}

#[test]
fn face_families_rejects_non_fonts() {
    let dir = std::env::temp_dir().join(format!("pbanki-font-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let short = dir.join("short.ttf");
    fs::write(&short, b"abc").unwrap();
    assert!(face_families(&short).is_err());

    let bogus = dir.join("bogus.ttc");
    let mut data = b"ttcf".to_vec();
    data.extend_from_slice(&[0, 1, 0, 0]);
    data.extend_from_slice(&u32::MAX.to_be_bytes());
    fs::write(&bogus, &data).unwrap();
    assert!(face_families(&bogus).is_err());
    let _ = fs::remove_dir_all(&dir);
}
