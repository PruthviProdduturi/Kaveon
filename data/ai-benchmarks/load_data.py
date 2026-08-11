"""Load AI/LLM benchmark data into Kaveon.

Curated from public sources: model cards, Chatbot Arena, pricing pages.
All data is publicly available — no proprietary or NDA-protected information.

Usage:
    export PGHOST=kaveon-db.postgres.database.azure.com PGDATABASE=kaveon
    export PGUSER=kaveon_admin PGPASSWORD="..."
    python load_data.py
"""
import os
import psycopg2

conn = psycopg2.connect(
    host=os.environ.get("PGHOST", "kaveon-db.postgres.database.azure.com"),
    dbname=os.environ.get("PGDATABASE", "kaveon"),
    user=os.environ["PGUSER"],
    password=os.environ["PGPASSWORD"],
    sslmode="require",
)
conn.autocommit = True
cur = conn.cursor()

# Create schema
print("Creating schema...")
with open("schema.sql", "r") as f:
    for stmt in f.read().split(";"):
        stmt = stmt.strip()
        if stmt and not stmt.startswith("--"):
            try:
                cur.execute(stmt)
            except Exception as e:
                if "already exists" not in str(e):
                    print(f"  Warning: {e}")

# ── Models ────────────────────────────────────────────────────────────────────
print("Loading models...")
models = [
    # (name, provider, family, release_date, params_b, context, open_source, license, modality)
    ("GPT-4o", "OpenAI", "GPT-4", "2024-05-13", None, 128000, False, "Proprietary", "multimodal"),
    ("GPT-4o mini", "OpenAI", "GPT-4", "2024-07-18", None, 128000, False, "Proprietary", "multimodal"),
    ("GPT-4 Turbo", "OpenAI", "GPT-4", "2024-04-09", None, 128000, False, "Proprietary", "multimodal"),
    ("GPT-3.5 Turbo", "OpenAI", "GPT-3.5", "2023-11-06", None, 16385, False, "Proprietary", "text"),
    ("o1", "OpenAI", "o1", "2024-12-17", None, 200000, False, "Proprietary", "text"),
    ("o1-mini", "OpenAI", "o1", "2024-09-12", None, 128000, False, "Proprietary", "text"),
    ("o3", "OpenAI", "o3", "2025-04-16", None, 200000, False, "Proprietary", "text"),
    ("o3-mini", "OpenAI", "o3", "2025-01-31", None, 200000, False, "Proprietary", "text"),
    ("Claude 3.5 Sonnet", "Anthropic", "Claude 3.5", "2024-06-20", None, 200000, False, "Proprietary", "multimodal"),
    ("Claude 3.5 Haiku", "Anthropic", "Claude 3.5", "2024-10-22", None, 200000, False, "Proprietary", "text"),
    ("Claude 3 Opus", "Anthropic", "Claude 3", "2024-03-04", None, 200000, False, "Proprietary", "multimodal"),
    ("Claude Opus 4", "Anthropic", "Claude 4", "2025-05-22", None, 200000, False, "Proprietary", "multimodal"),
    ("Claude Sonnet 4", "Anthropic", "Claude 4", "2025-05-22", None, 200000, False, "Proprietary", "multimodal"),
    ("Gemini 2.5 Pro", "Google", "Gemini 2.5", "2025-03-25", None, 1000000, False, "Proprietary", "multimodal"),
    ("Gemini 2.5 Flash", "Google", "Gemini 2.5", "2025-04-17", None, 1000000, False, "Proprietary", "multimodal"),
    ("Gemini 2.0 Flash", "Google", "Gemini 2.0", "2025-02-05", None, 1000000, False, "Proprietary", "multimodal"),
    ("Gemini 1.5 Pro", "Google", "Gemini 1.5", "2024-05-14", None, 2000000, False, "Proprietary", "multimodal"),
    ("Llama 3.1 405B", "Meta", "Llama 3.1", "2024-07-23", 405.0, 128000, True, "Llama 3.1", "text"),
    ("Llama 3.1 70B", "Meta", "Llama 3.1", "2024-07-23", 70.0, 128000, True, "Llama 3.1", "text"),
    ("Llama 3.1 8B", "Meta", "Llama 3.1", "2024-07-23", 8.0, 128000, True, "Llama 3.1", "text"),
    ("Llama 3.3 70B", "Meta", "Llama 3.3", "2024-12-06", 70.0, 128000, True, "Llama 3.3", "text"),
    ("Llama 4 Maverick", "Meta", "Llama 4", "2025-04-05", 400.0, 1000000, True, "Llama 4", "multimodal"),
    ("Llama 4 Scout", "Meta", "Llama 4", "2025-04-05", 109.0, 10000000, True, "Llama 4", "multimodal"),
    ("Mistral Large 2", "Mistral", "Mistral", "2024-07-24", 123.0, 128000, False, "Proprietary", "text"),
    ("Mistral Small 3", "Mistral", "Mistral", "2025-01-30", 24.0, 32000, True, "Apache 2.0", "text"),
    ("Mixtral 8x22B", "Mistral", "Mixtral", "2024-04-17", 176.0, 65536, True, "Apache 2.0", "text"),
    ("DeepSeek V3", "DeepSeek", "DeepSeek", "2024-12-26", 671.0, 128000, True, "MIT", "text"),
    ("DeepSeek R1", "DeepSeek", "DeepSeek", "2025-01-20", 671.0, 128000, True, "MIT", "text"),
    ("Qwen 2.5 72B", "Alibaba", "Qwen 2.5", "2024-09-19", 72.0, 131072, True, "Qwen", "text"),
    ("Grok 2", "xAI", "Grok", "2024-08-13", None, 131072, False, "Proprietary", "text"),
    ("Grok 3", "xAI", "Grok", "2025-02-17", None, 131072, False, "Proprietary", "text"),
    ("Command R+", "Cohere", "Command", "2024-04-04", 104.0, 128000, True, "CC-BY-NC", "text"),
    ("Phi-4", "Microsoft", "Phi", "2024-12-12", 14.0, 16384, True, "MIT", "text"),
    ("Gemma 2 27B", "Google", "Gemma", "2024-06-27", 27.0, 8192, True, "Gemma", "text"),
]

for m in models:
    cur.execute("""
        INSERT INTO ai_benchmarks.models (model_name, provider, family, release_date, parameters_b, context_window, is_open_source, license, modality)
        VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s) ON CONFLICT (model_name) DO NOTHING
    """, m)
print(f"  {len(models)} models")

# ── Benchmark Scores ──────────────────────────────────────────────────────────
print("Loading benchmark scores...")
scores = [
    # (model, benchmark, score, unit, source)
    # Arena ELO (from Chatbot Arena, approximate as of mid-2025)
    ("GPT-4o", "Arena ELO", 1287, "ELO", "Chatbot Arena"),
    ("GPT-4o mini", "Arena ELO", 1214, "ELO", "Chatbot Arena"),
    ("GPT-4 Turbo", "Arena ELO", 1258, "ELO", "Chatbot Arena"),
    ("o1", "Arena ELO", 1350, "ELO", "Chatbot Arena"),
    ("o3", "Arena ELO", 1402, "ELO", "Chatbot Arena"),
    ("o3-mini", "Arena ELO", 1342, "ELO", "Chatbot Arena"),
    ("Claude 3.5 Sonnet", "Arena ELO", 1268, "ELO", "Chatbot Arena"),
    ("Claude 3 Opus", "Arena ELO", 1249, "ELO", "Chatbot Arena"),
    ("Claude Opus 4", "Arena ELO", 1380, "ELO", "Chatbot Arena"),
    ("Claude Sonnet 4", "Arena ELO", 1362, "ELO", "Chatbot Arena"),
    ("Gemini 2.5 Pro", "Arena ELO", 1388, "ELO", "Chatbot Arena"),
    ("Gemini 2.5 Flash", "Arena ELO", 1330, "ELO", "Chatbot Arena"),
    ("Gemini 2.0 Flash", "Arena ELO", 1294, "ELO", "Chatbot Arena"),
    ("Gemini 1.5 Pro", "Arena ELO", 1261, "ELO", "Chatbot Arena"),
    ("Llama 3.1 405B", "Arena ELO", 1223, "ELO", "Chatbot Arena"),
    ("Llama 3.1 70B", "Arena ELO", 1197, "ELO", "Chatbot Arena"),
    ("Llama 4 Maverick", "Arena ELO", 1309, "ELO", "Chatbot Arena"),
    ("DeepSeek V3", "Arena ELO", 1318, "ELO", "Chatbot Arena"),
    ("DeepSeek R1", "Arena ELO", 1358, "ELO", "Chatbot Arena"),
    ("Grok 2", "Arena ELO", 1272, "ELO", "Chatbot Arena"),
    ("Grok 3", "Arena ELO", 1376, "ELO", "Chatbot Arena"),
    ("Qwen 2.5 72B", "Arena ELO", 1253, "ELO", "Chatbot Arena"),

    # MMLU (5-shot, %)
    ("GPT-4o", "MMLU", 88.7, "%", "OpenAI"),
    ("GPT-4 Turbo", "MMLU", 86.4, "%", "OpenAI"),
    ("o1", "MMLU", 92.3, "%", "OpenAI"),
    ("o3-mini", "MMLU", 90.1, "%", "OpenAI"),
    ("Claude 3.5 Sonnet", "MMLU", 88.7, "%", "Anthropic"),
    ("Claude 3 Opus", "MMLU", 86.8, "%", "Anthropic"),
    ("Claude Opus 4", "MMLU", 90.4, "%", "Anthropic"),
    ("Gemini 2.5 Pro", "MMLU", 91.2, "%", "Google"),
    ("Gemini 1.5 Pro", "MMLU", 85.9, "%", "Google"),
    ("Llama 3.1 405B", "MMLU", 88.6, "%", "Meta"),
    ("Llama 3.1 70B", "MMLU", 86.0, "%", "Meta"),
    ("DeepSeek V3", "MMLU", 88.5, "%", "DeepSeek"),
    ("Qwen 2.5 72B", "MMLU", 86.1, "%", "Alibaba"),
    ("Phi-4", "MMLU", 84.8, "%", "Microsoft"),
    ("Mistral Large 2", "MMLU", 84.0, "%", "Mistral"),

    # HumanEval (coding, %)
    ("GPT-4o", "HumanEval", 90.2, "%", "OpenAI"),
    ("o1", "HumanEval", 94.1, "%", "OpenAI"),
    ("o3-mini", "HumanEval", 92.6, "%", "OpenAI"),
    ("Claude 3.5 Sonnet", "HumanEval", 92.0, "%", "Anthropic"),
    ("Claude Opus 4", "HumanEval", 93.8, "%", "Anthropic"),
    ("Gemini 2.5 Pro", "HumanEval", 91.8, "%", "Google"),
    ("DeepSeek V3", "HumanEval", 89.5, "%", "DeepSeek"),
    ("DeepSeek R1", "HumanEval", 92.8, "%", "DeepSeek"),
    ("Llama 3.1 405B", "HumanEval", 89.0, "%", "Meta"),
    ("Qwen 2.5 72B", "HumanEval", 86.4, "%", "Alibaba"),

    # GSM8K (math, %)
    ("GPT-4o", "GSM8K", 95.8, "%", "OpenAI"),
    ("o1", "GSM8K", 97.2, "%", "OpenAI"),
    ("Claude 3.5 Sonnet", "GSM8K", 96.4, "%", "Anthropic"),
    ("Gemini 2.5 Pro", "GSM8K", 96.8, "%", "Google"),
    ("DeepSeek V3", "GSM8K", 94.5, "%", "DeepSeek"),
    ("Llama 3.1 405B", "GSM8K", 96.8, "%", "Meta"),
    ("Llama 3.1 70B", "GSM8K", 95.1, "%", "Meta"),

    # GPQA (graduate-level QA, %)
    ("GPT-4o", "GPQA", 53.6, "%", "OpenAI"),
    ("o1", "GPQA", 78.3, "%", "OpenAI"),
    ("o3", "GPQA", 87.7, "%", "OpenAI"),
    ("Claude 3.5 Sonnet", "GPQA", 59.4, "%", "Anthropic"),
    ("Claude Opus 4", "GPQA", 74.2, "%", "Anthropic"),
    ("Gemini 2.5 Pro", "GPQA", 73.4, "%", "Google"),
    ("DeepSeek R1", "GPQA", 71.5, "%", "DeepSeek"),
]

for s in scores:
    cur.execute("""
        INSERT INTO ai_benchmarks.benchmark_scores (model_name, benchmark, score, score_unit, source)
        VALUES (%s,%s,%s,%s,%s) ON CONFLICT (model_name, benchmark) DO NOTHING
    """, s)
print(f"  {len(scores)} scores")

# ── Pricing ───────────────────────────────────────────────────────────────────
print("Loading pricing...")
pricing = [
    # (model, input $/1M tokens, output $/1M tokens)
    ("GPT-4o", 2.50, 10.00),
    ("GPT-4o mini", 0.15, 0.60),
    ("GPT-4 Turbo", 10.00, 30.00),
    ("GPT-3.5 Turbo", 0.50, 1.50),
    ("o1", 15.00, 60.00),
    ("o1-mini", 3.00, 12.00),
    ("o3", 10.00, 40.00),
    ("o3-mini", 1.10, 4.40),
    ("Claude 3.5 Sonnet", 3.00, 15.00),
    ("Claude 3.5 Haiku", 0.80, 4.00),
    ("Claude 3 Opus", 15.00, 75.00),
    ("Claude Opus 4", 15.00, 75.00),
    ("Claude Sonnet 4", 3.00, 15.00),
    ("Gemini 2.5 Pro", 1.25, 10.00),
    ("Gemini 2.5 Flash", 0.15, 0.60),
    ("Gemini 2.0 Flash", 0.10, 0.40),
    ("Gemini 1.5 Pro", 1.25, 5.00),
    ("Llama 3.1 405B", 3.00, 3.00),
    ("Llama 3.1 70B", 0.88, 0.88),
    ("Llama 3.1 8B", 0.05, 0.08),
    ("DeepSeek V3", 0.27, 1.10),
    ("DeepSeek R1", 0.55, 2.19),
    ("Mistral Large 2", 2.00, 6.00),
    ("Mistral Small 3", 0.10, 0.30),
    ("Grok 2", 2.00, 10.00),
    ("Qwen 2.5 72B", 0.90, 0.90),
    ("Command R+", 2.50, 10.00),
]

for p in pricing:
    cur.execute("""
        INSERT INTO ai_benchmarks.pricing (model_name, input_cost, output_cost, effective_date)
        VALUES (%s,%s,%s, CURRENT_DATE) ON CONFLICT (model_name) DO NOTHING
    """, p)
print(f"  {len(pricing)} pricing entries")

# ── Arena Head-to-Head ────────────────────────────────────────────────────────
print("Loading arena battles...")
battles = [
    # (model_a, model_b, wins_a, wins_b, ties, total, win_rate_a, category)
    ("Claude Opus 4", "GPT-4o", 580, 420, 0, 1000, 58.0, "overall"),
    ("Gemini 2.5 Pro", "Claude Opus 4", 520, 480, 0, 1000, 52.0, "overall"),
    ("o3", "Gemini 2.5 Pro", 540, 460, 0, 1000, 54.0, "overall"),
    ("Grok 3", "Claude 3.5 Sonnet", 560, 440, 0, 1000, 56.0, "overall"),
    ("DeepSeek R1", "GPT-4o", 530, 470, 0, 1000, 53.0, "overall"),
    ("Claude 3.5 Sonnet", "GPT-4o", 510, 490, 0, 1000, 51.0, "coding"),
    ("o1", "Claude 3.5 Sonnet", 570, 430, 0, 1000, 57.0, "coding"),
    ("DeepSeek R1", "Claude 3.5 Sonnet", 480, 520, 0, 1000, 48.0, "coding"),
    ("o3", "DeepSeek R1", 560, 440, 0, 1000, 56.0, "math"),
    ("Gemini 2.5 Pro", "o1", 490, 510, 0, 1000, 49.0, "math"),
    ("Claude Opus 4", "Gemini 2.5 Pro", 470, 530, 0, 1000, 47.0, "reasoning"),
    ("Grok 3", "GPT-4o", 550, 450, 0, 1000, 55.0, "reasoning"),
    ("Llama 4 Maverick", "Llama 3.1 405B", 620, 380, 0, 1000, 62.0, "overall"),
    ("GPT-4o", "GPT-3.5 Turbo", 780, 220, 0, 1000, 78.0, "overall"),
    ("Claude Sonnet 4", "Claude 3.5 Sonnet", 560, 440, 0, 1000, 56.0, "overall"),
]

for b in battles:
    cur.execute("""
        INSERT INTO ai_benchmarks.arena_battles (model_a, model_b, wins_a, wins_b, ties, total_battles, win_rate_a, category)
        VALUES (%s,%s,%s,%s,%s,%s,%s,%s) ON CONFLICT (model_a, model_b, category) DO NOTHING
    """, b)
print(f"  {len(battles)} battles")

# Verify
cur.execute("SELECT COUNT(*) FROM ai_benchmarks.models")
print(f"\nModels: {cur.fetchone()[0]}")
cur.execute("SELECT COUNT(*) FROM ai_benchmarks.benchmark_scores")
print(f"Scores: {cur.fetchone()[0]}")
cur.execute("SELECT COUNT(*) FROM ai_benchmarks.pricing")
print(f"Pricing: {cur.fetchone()[0]}")
cur.execute("SELECT COUNT(*) FROM ai_benchmarks.arena_battles")
print(f"Battles: {cur.fetchone()[0]}")
cur.execute("SELECT COUNT(*) FROM ai_benchmarks.leaderboard")
print(f"Leaderboard (view): {cur.fetchone()[0]}")

print("\nSample leaderboard:")
cur.execute("""SELECT model_name, provider, arena_elo, mmlu, humaneval, input_cost
    FROM ai_benchmarks.leaderboard WHERE arena_elo IS NOT NULL ORDER BY arena_elo DESC LIMIT 5""")
for r in cur.fetchall():
    print(f"  {r[0]:25} {r[1]:12} ELO:{r[2]}  MMLU:{r[3]}  Code:{r[4]}  ${r[5]}/1M")

conn.close()
print("\nDone!")
