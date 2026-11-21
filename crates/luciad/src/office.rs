//! Office Open XML(docx·xlsx·pptx)에서 글만 뽑는다(결정 0005). 서식·그림은 버린다.
//!
//! 압축 폭탄을 막으려고 항목 수, 항목 하나의 풀린 크기, 전체 풀린 크기에 상한을 둔다.

use std::io::{Cursor, Read};

use zip::ZipArchive;

const MAX_ENTRIES: usize = 5_000;
const MAX_ENTRY_BYTES: u64 = 20 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 60 * 1024 * 1024;

struct Reader {
    zip: ZipArchive<Cursor<Vec<u8>>>,
    total: u64,
}

impl Reader {
    fn open(bytes: &[u8]) -> Result<Self, String> {
        let zip =
            ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|e| format!("압축 형식: {e}"))?;
        if zip.len() > MAX_ENTRIES {
            return Err("항목이 너무 많다".into());
        }
        Ok(Self { zip, total: 0 })
    }

    fn names(&mut self) -> Vec<String> {
        (0..self.zip.len())
            .filter_map(|i| {
                self.zip
                    .by_index(i)
                    .ok()
                    .and_then(|f| f.name().ok().map(|n| n.into_owned()))
            })
            .collect()
    }

    /// 항목 하나를 글로 읽는다(없으면 `None`).
    fn read(&mut self, name: &str) -> Result<Option<String>, String> {
        let entry = match self.zip.by_name(name) {
            Ok(entry) => entry,
            Err(_) => return Ok(None),
        };
        let mut buf = Vec::new();
        entry
            .take(MAX_ENTRY_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|e| format!("압축 풀기: {e}"))?;
        if buf.len() as u64 > MAX_ENTRY_BYTES {
            return Err("풀린 항목이 너무 크다".into());
        }
        self.total += buf.len() as u64;
        if self.total > MAX_TOTAL_BYTES {
            return Err("풀린 크기가 너무 크다".into());
        }
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }
}

/// XML 엔티티를 푼다.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 태그를 걷어 내고 글만 남긴다. `breaks`의 닫는 태그(문단)와 줄바꿈 태그에서 줄을 바꾼다.
/// 글은 `text_tags` 안의 것만 모은다(속성·숨은 값이 섞이지 않게).
fn xml_text(xml: &str, text_tags: &[&str], paragraph: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = xml;
    let mut in_text = false;
    while let Some(lt) = rest.find('<') {
        if in_text {
            out.push_str(&unescape(&rest[..lt]));
        }
        let Some(gt) = rest[lt..].find('>') else {
            break;
        };
        let tag = &rest[lt + 1..lt + gt];
        rest = &rest[lt + gt + 1..];
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if text_tags.contains(&name) {
            in_text = !closing && !self_closing;
        } else if closing && paragraph.contains(&name) {
            out.push('\n');
        } else if name.ends_with(":tab") {
            out.push('\t');
        } else if name.ends_with(":br") || name.ends_with(":cr") {
            out.push('\n');
        }
    }
    out
}

/// 숫자로 끝나는 항목 이름을 숫자 순으로(slide2가 slide10보다 앞에).
fn numbered(mut names: Vec<String>, prefix: &str, suffix: &str) -> Vec<String> {
    names.retain(|n| n.starts_with(prefix) && n.ends_with(suffix));
    names.sort_by_key(|n| {
        n[prefix.len()..n.len() - suffix.len()]
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    names
}

fn tidy(text: String) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_owned()
}

/// docx·pptx·xlsx의 글. 다른 확장자는 거부한다.
pub fn extract_text(bytes: &[u8], ext: &str) -> Result<String, String> {
    let mut zip = Reader::open(bytes)?;
    let text = match ext {
        "docx" => {
            let xml = zip
                .read("word/document.xml")?
                .ok_or("word/document.xml이 없다")?;
            xml_text(&xml, &["w:t"], &["w:p"])
        }
        "pptx" => {
            let mut out = String::new();
            let slides = numbered(zip.names(), "ppt/slides/slide", ".xml");
            for (i, name) in slides.iter().enumerate() {
                if let Some(xml) = zip.read(name)? {
                    out.push_str(&format!("--- 슬라이드 {} ---\n", i + 1));
                    out.push_str(&xml_text(&xml, &["a:t"], &["a:p"]));
                    out.push('\n');
                }
            }
            out
        }
        "xlsx" => xlsx_text(&mut zip)?,
        other => return Err(format!("글을 뽑을 수 없는 형식: .{other}")),
    };
    let text = tidy(text);
    if text.is_empty() {
        Err("글이 없다".into())
    } else {
        Ok(text)
    }
}

fn xlsx_text(zip: &mut Reader) -> Result<String, String> {
    // 공유 문자열: <si> 하나가 문자열 하나(안의 <t>를 이어 붙인다).
    let shared: Vec<String> = match zip.read("xl/sharedStrings.xml")? {
        Some(xml) => xml
            .split("<si>")
            .skip(1)
            .map(|si| {
                let si = si.split("</si>").next().unwrap_or("");
                xml_text(si, &["t"], &[]).replace('\n', " ")
            })
            .collect(),
        None => Vec::new(),
    };
    let sheet_names: Vec<String> = zip
        .read("xl/workbook.xml")?
        .map(|xml| {
            xml.split("<sheet ")
                .skip(1)
                .filter_map(|s| {
                    let start = s.find("name=\"")? + 6;
                    let end = s[start..].find('"')?;
                    Some(unescape(&s[start..start + end]))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out = String::new();
    let sheets = numbered(zip.names(), "xl/worksheets/sheet", ".xml");
    for (i, name) in sheets.iter().enumerate() {
        let Some(xml) = zip.read(name)? else { continue };
        out.push_str(&format!(
            "--- 시트 {} ---\n",
            sheet_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| (i + 1).to_string())
        ));
        for row in xml.split("<row").skip(1) {
            let row = row.split("</row>").next().unwrap_or("");
            let mut cells = Vec::new();
            for cell in row.split("<c ").skip(1) {
                let (attrs, body) = cell.split_once('>').unwrap_or((cell, ""));
                if attrs.ends_with('/') {
                    continue;
                }
                let body = body.split("</c>").next().unwrap_or("");
                let value = body
                    .split("<v>")
                    .nth(1)
                    .and_then(|v| v.split("</v>").next())
                    .map(unescape);
                let text = if attrs.contains("t=\"s\"") {
                    value
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .and_then(|i| shared.get(i).cloned())
                } else if attrs.contains("t=\"inlineStr\"") {
                    Some(xml_text(body, &["t"], &[]))
                } else {
                    value
                };
                if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
                    cells.push(t);
                }
            }
            if !cells.is_empty() {
                out.push_str(&cells.join("\t"));
                out.push('\n');
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
        // 시험용 zip을 손으로 만든다(무압축, stored). 쓰기 기능을 켜지 않으려고.
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, body) in entries {
            let offset = out.len() as u32;
            let crc = crc32(body.as_bytes());
            let header = |sig: u32, central: bool| {
                let mut h = Vec::new();
                h.write_all(&sig.to_le_bytes()).unwrap();
                if central {
                    h.write_all(&20u16.to_le_bytes()).unwrap();
                }
                h.write_all(&20u16.to_le_bytes()).unwrap();
                h.write_all(&0u16.to_le_bytes()).unwrap(); // flags
                h.write_all(&0u16.to_le_bytes()).unwrap(); // stored
                h.write_all(&0u32.to_le_bytes()).unwrap(); // time/date
                h.write_all(&crc.to_le_bytes()).unwrap();
                h.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
                h.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
                h.write_all(&(name.len() as u16).to_le_bytes()).unwrap();
                h.write_all(&0u16.to_le_bytes()).unwrap(); // extra
                if central {
                    h.write_all(&0u16.to_le_bytes()).unwrap(); // comment
                    h.write_all(&0u16.to_le_bytes()).unwrap(); // disk
                    h.write_all(&0u16.to_le_bytes()).unwrap(); // internal attrs
                    h.write_all(&0u32.to_le_bytes()).unwrap(); // external attrs
                    h.write_all(&offset.to_le_bytes()).unwrap();
                }
                h.write_all(name.as_bytes()).unwrap();
                h
            };
            out.extend(header(0x04034b50, false));
            out.extend(body.as_bytes());
            central.extend(header(0x02014b50, true));
        }
        let cd_offset = out.len() as u32;
        out.extend(&central);
        out.write_all(&0x06054b50u32.to_le_bytes()).unwrap();
        out.write_all(&[0, 0, 0, 0]).unwrap();
        out.write_all(&(entries.len() as u16).to_le_bytes())
            .unwrap();
        out.write_all(&(entries.len() as u16).to_le_bytes())
            .unwrap();
        out.write_all(&(central.len() as u32).to_le_bytes())
            .unwrap();
        out.write_all(&cd_offset.to_le_bytes()).unwrap();
        out.write_all(&0u16.to_le_bytes()).unwrap();
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn docx_paragraphs_and_entities() {
        let doc = zip_of(&[(
            "word/document.xml",
            r#"<w:document><w:body><w:p><w:r><w:t>회의 &amp; 정리</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">둘째 </w:t><w:tab/><w:t>줄</w:t></w:r></w:p></w:body></w:document>"#,
        )]);
        assert_eq!(
            extract_text(&doc, "docx").unwrap(),
            "회의 & 정리\n둘째 \t줄"
        );
    }

    #[test]
    fn pptx_slides_in_order() {
        let deck = zip_of(&[
            (
                "ppt/slides/slide10.xml",
                r#"<p:sld><a:p><a:r><a:t>열</a:t></a:r></a:p></p:sld>"#,
            ),
            (
                "ppt/slides/slide2.xml",
                r#"<p:sld><a:p><a:r><a:t>둘</a:t></a:r></a:p></p:sld>"#,
            ),
        ]);
        let text = extract_text(&deck, "pptx").unwrap();
        assert!(text.find("둘").unwrap() < text.find("열").unwrap());
        assert!(text.contains("--- 슬라이드 1 ---"));
    }

    #[test]
    fn xlsx_cells_with_shared_strings() {
        let book = zip_of(&[
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="예산" sheetId="1"/></sheets></workbook>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><t>항목</t></si><si><t>식비</t></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>금액</t></is></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2"><v>12000</v></c></row></sheetData></worksheet>"#,
            ),
        ]);
        assert_eq!(
            extract_text(&book, "xlsx").unwrap(),
            "--- 시트 예산 ---\n항목\t금액\n식비\t12000"
        );
    }

    #[test]
    fn broken_or_unknown_inputs_are_rejected() {
        assert!(extract_text(b"PK\x03\x04garbage", "docx").is_err());
        assert!(extract_text(&zip_of(&[("a.txt", "x")]), "docx").is_err());
        assert!(extract_text(&zip_of(&[("a.txt", "x")]), "zip").is_err());
    }

    /// 실제 Office 파일처럼 deflate로 압축한 경우.
    #[test]
    fn deflated_docx_is_read() {
        use zip::write::SimpleFileOptions;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            w.start_file("word/document.xml", opts).unwrap();
            let body = format!(
                "<w:document><w:body>{}</w:body></w:document>",
                "<w:p><w:r><w:t>반복 문단</w:t></w:r></w:p>".repeat(200)
            );
            w.write_all(body.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        let bytes = buf.into_inner();
        let text = extract_text(&bytes, "docx").unwrap();
        assert_eq!(text.lines().count(), 200);
        assert!(bytes.len() < 2000, "압축됐다: {}", bytes.len());
    }
}
