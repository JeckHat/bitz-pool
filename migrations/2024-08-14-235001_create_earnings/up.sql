CREATE TABLE earnings (
  id INT NOT NULL AUTO_INCREMENT PRIMARY KEY,
  miner_id INT NOT NULL,
  pool_id INT NOT NULL,
  challenge_id INT NOT NULL,
  amount BIGINT UNSIGNED DEFAULT 0 NOT NULL,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP NOT NULL
);
CREATE INDEX idx_earnings_miner_challenge_created ON earnings(miner_id, challenge_id, created_at);