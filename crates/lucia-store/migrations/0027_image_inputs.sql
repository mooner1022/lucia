-- 이미지 편집(결정 0046 후속): 요청에 넣은 그림 수. 0이면 새로 그린 것이다.
ALTER TABLE generated_images ADD COLUMN inputs INTEGER NOT NULL DEFAULT 0;
