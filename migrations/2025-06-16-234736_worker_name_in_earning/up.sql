-- Your SQL goes here
ALTER TABLE earnings ADD COLUMN worker_name VARCHAR(36) NOT NULL DEFAULT "-" AFTER difficulty;