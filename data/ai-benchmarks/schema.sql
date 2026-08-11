-- AI/LLM Benchmarks schema
-- Kaveon showcase: model performance, arena rankings, costs, capabilities
CREATE SCHEMA IF NOT EXISTS ai_benchmarks;

-- Core model registry
CREATE TABLE ai_benchmarks.models (
    id              SERIAL PRIMARY KEY,
    model_name      VARCHAR(100) NOT NULL UNIQUE,
    provider        VARCHAR(50) NOT NULL,        -- OpenAI, Anthropic, Google, Meta, Mistral, etc.
    family          VARCHAR(50),                 -- GPT-4, Claude, Gemini, Llama, etc.
    release_date    DATE,
    parameters_b    DECIMAL(10,1),               -- billions of parameters (NULL if undisclosed)
    context_window  INTEGER,                     -- max tokens
    is_open_source  BOOLEAN DEFAULT FALSE,
    license         VARCHAR(50),                 -- MIT, Apache 2.0, Proprietary, etc.
    modality        VARCHAR(20) DEFAULT 'text',  -- text, multimodal, code
    created_at      TIMESTAMPTZ DEFAULT NOW()
);

-- Benchmark scores (MMLU, HumanEval, GSM8K, etc.)
CREATE TABLE ai_benchmarks.benchmark_scores (
    id              SERIAL PRIMARY KEY,
    model_name      VARCHAR(100) NOT NULL REFERENCES ai_benchmarks.models(model_name),
    benchmark       VARCHAR(50) NOT NULL,        -- MMLU, HumanEval, GSM8K, Arena ELO, MT-Bench, etc.
    score           DECIMAL(10,2) NOT NULL,
    score_unit      VARCHAR(20) DEFAULT '%',     -- %, ELO, points
    eval_date       DATE,
    source          VARCHAR(100),                -- model card, paper, arena, etc.
    UNIQUE (model_name, benchmark)
);

-- Pricing
CREATE TABLE ai_benchmarks.pricing (
    id              SERIAL PRIMARY KEY,
    model_name      VARCHAR(100) NOT NULL REFERENCES ai_benchmarks.models(model_name),
    input_cost      DECIMAL(10,4),               -- $ per 1M input tokens
    output_cost     DECIMAL(10,4),               -- $ per 1M output tokens
    effective_date  DATE,
    UNIQUE (model_name)
);

-- Arena head-to-head battles (aggregated win rates)
CREATE TABLE ai_benchmarks.arena_battles (
    id              SERIAL PRIMARY KEY,
    model_a         VARCHAR(100) NOT NULL,
    model_b         VARCHAR(100) NOT NULL,
    wins_a          INTEGER NOT NULL,
    wins_b          INTEGER NOT NULL,
    ties            INTEGER DEFAULT 0,
    total_battles   INTEGER NOT NULL,
    win_rate_a      DECIMAL(5,2),
    category        VARCHAR(30) DEFAULT 'overall', -- overall, coding, math, reasoning, creative
    UNIQUE (model_a, model_b, category)
);

-- Indexes
CREATE INDEX idx_scores_model ON ai_benchmarks.benchmark_scores(model_name);
CREATE INDEX idx_scores_bench ON ai_benchmarks.benchmark_scores(benchmark);
CREATE INDEX idx_pricing_model ON ai_benchmarks.pricing(model_name);
CREATE INDEX idx_arena_models ON ai_benchmarks.arena_battles(model_a, model_b);

-- View: Model leaderboard with all scores pivoted
CREATE OR REPLACE VIEW ai_benchmarks.leaderboard AS
SELECT
    m.model_name,
    m.provider,
    m.family,
    m.release_date,
    m.parameters_b,
    m.context_window,
    m.is_open_source,
    p.input_cost,
    p.output_cost,
    MAX(CASE WHEN bs.benchmark = 'Arena ELO' THEN bs.score END) AS arena_elo,
    MAX(CASE WHEN bs.benchmark = 'MMLU' THEN bs.score END) AS mmlu,
    MAX(CASE WHEN bs.benchmark = 'HumanEval' THEN bs.score END) AS humaneval,
    MAX(CASE WHEN bs.benchmark = 'GSM8K' THEN bs.score END) AS gsm8k,
    MAX(CASE WHEN bs.benchmark = 'MT-Bench' THEN bs.score END) AS mt_bench,
    MAX(CASE WHEN bs.benchmark = 'GPQA' THEN bs.score END) AS gpqa,
    MAX(CASE WHEN bs.benchmark = 'MATH' THEN bs.score END) AS math_score,
    MAX(CASE WHEN bs.benchmark = 'ARC-Challenge' THEN bs.score END) AS arc_challenge
FROM ai_benchmarks.models m
LEFT JOIN ai_benchmarks.benchmark_scores bs ON m.model_name = bs.model_name
LEFT JOIN ai_benchmarks.pricing p ON m.model_name = p.model_name
GROUP BY m.model_name, m.provider, m.family, m.release_date, m.parameters_b,
         m.context_window, m.is_open_source, p.input_cost, p.output_cost;
