-- 캐릭터 파일 변경 기록(결정 0009): 관리 페이지에서 고칠 때마다 전체 내용을 남긴다(되돌리기용).
CREATE TABLE character_revisions (
    id      INTEGER PRIMARY KEY,
    at      TEXT NOT NULL,
    note    TEXT NOT NULL,          -- 바꾼 까닭(사용자 입력) 또는 데몬이 붙인 설명
    content TEXT NOT NULL
);
