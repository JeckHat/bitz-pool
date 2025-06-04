CREATE TABLE submissions (
  id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,
  miner_id INT NOT NULL,
  challenge_id INT NOT NULL,
  difficulty TINYINT NOT NULL,
  nonce BIGINT UNSIGNED NOT NULL,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP NOT NULL
);
CREATE INDEX idx_submissions_miner_challenge_created ON submissions(miner_id, challenge_id, created_at);