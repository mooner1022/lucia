//! 한국어 전문 검색 토크나이저 비교(PLAN §2.4). 스키마를 정하기 전에 돌린 시험을 회귀 테스트로 남긴다.
//!
//! 비교: trigram 토크나이저 vs unicode61 + 접두사 질의. 두 글자 질의("과제", "시험")와
//! 조사가 붙은 낱말("과제를"), 합성어 안의 낱말("기말과제")을 본다.

use rusqlite::Connection;

const DOCS: &[&str] = &[
    "운영체제 과제를 금요일까지 제출해야 한다", // 1: 과제+조사
    "기말과제 마감이 다음 주 수요일로 바뀌었다", // 2: 합성어 안의 과제
    "중간시험 범위는 3장부터 6장까지다",        // 3: 합성어 안의 시험
    "시험 일정 공지가 올라왔다",                // 4: 시험 단독
    "점심 약속은 취소됐다",                     // 5: 무관
];

fn setup(tokenizer: &str) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE docs USING fts5(body, tokenize = '{tokenizer}');"
    ))
    .unwrap();
    for (i, body) in DOCS.iter().enumerate() {
        conn.execute(
            "INSERT INTO docs(rowid, body) VALUES (?1, ?2)",
            rusqlite::params![i as i64 + 1, body],
        )
        .unwrap();
    }
    conn
}

fn query(conn: &Connection, sql: &str, arg: &str) -> Vec<i64> {
    let mut stmt = conn.prepare(sql).unwrap();
    let mut rows: Vec<i64> = stmt
        .query_map([arg], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows.sort();
    rows
}

#[test]
fn trigram_misses_two_syllable_match_but_like_finds_it() {
    let conn = setup("trigram");
    let matched = query(&conn, "SELECT rowid FROM docs WHERE docs MATCH ?1", "과제");
    let liked = query(
        &conn,
        "SELECT rowid FROM docs WHERE body LIKE '%' || ?1 || '%'",
        "과제",
    );
    let three = query(
        &conn,
        "SELECT rowid FROM docs WHERE docs MATCH ?1",
        "과제를",
    );
    println!("trigram MATCH 과제={matched:?} LIKE 과제={liked:?} MATCH 과제를={three:?}");
    // 두 글자는 trigram MATCH로 못 찾는다(3글자 미만).
    assert!(matched.is_empty());
    // LIKE는 합성어 안까지 찾는다.
    assert_eq!(liked, vec![1, 2]);
    // 세 글자 이상은 MATCH로 찾는다.
    assert_eq!(three, vec![1]);
}

#[test]
fn unicode61_prefix_finds_particles_but_not_compounds() {
    let conn = setup("unicode61");
    let prefix = query(&conn, "SELECT rowid FROM docs WHERE docs MATCH ?1", "과제*");
    let exact = query(&conn, "SELECT rowid FROM docs WHERE docs MATCH ?1", "과제");
    let exam = query(&conn, "SELECT rowid FROM docs WHERE docs MATCH ?1", "시험*");
    println!("unicode61 과제*={prefix:?} 과제={exact:?} 시험*={exam:?}");
    // 조사가 붙은 "과제를"은 접두사로 찾지만, 합성어 "기말과제"는 못 찾는다.
    assert_eq!(prefix, vec![1]);
    assert!(exact.is_empty());
    assert_eq!(exam, vec![4]);
}
