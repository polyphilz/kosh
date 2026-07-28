#![cfg(target_os = "macos")]

use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn isolated_pdf_worker_extracts_a_bounded_response() {
    let response = run_worker(
        &["--kosh-pdf-extraction-worker", "1"],
        &single_page_pdf("Hi"),
    );
    assert_eq!(response["operation"], "EXTRACTION");
    let pages = response["result"]["Ok"]
        .as_array()
        .unwrap_or_else(|| panic!("worker returned an extraction error: {response}"));
    let page = &pages[0];
    assert_eq!(page["pageNumber"], 1);
    assert!(
        page["result"]["Ok"][1]
            .as_str()
            .expect("page text")
            .contains("Hi"),
        "short native text must survive the isolated extraction path"
    );
}

#[test]
fn isolated_pdf_worker_inspects_untrusted_input() {
    let response = run_worker(&["--kosh-pdf-inspection-worker"], &single_page_pdf("Hi"));
    assert_eq!(response["operation"], "INSPECTION");
    assert_eq!(response["result"]["Ok"], 1);
}

fn run_worker(arguments: &[&str], pdf: &[u8]) -> serde_json::Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kosh"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn PDF extraction worker");
    child
        .stdin
        .take()
        .expect("worker stdin")
        .write_all(pdf)
        .expect("send PDF fixture");
    let output = child.wait_with_output().expect("wait for PDF worker");
    assert!(
        output.status.success(),
        "worker failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() < 32 * 1024 * 1024);
    serde_json::from_slice(&output.stdout).expect("worker JSON response")
}

fn single_page_pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    let mut bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    bytes.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    bytes
}
