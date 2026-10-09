# ClickBench evidence validation

Campaign: `docs\qualification\clickbench\runs\rounds-2026-09-17`  
Rounds: Kaveon 5, Trino 5  
Statements: 43  
Latency wins: Kaveon 24, Trino 19, equal 0  
Exact result hashes equal: 28 / 43 shared statements

P50/P95 below are calculated over every recorded per-round sample (normally 15 samples per engine).

| Query | Kaveon p50/p95 s | Trino p50/p95 s | Trino ÷ Kaveon | Rows K/T | Exact hash | Result |
|---|---:|---:|---:|---:|---|---|
| `q01` | 0.857 / 1.226 | 4.502 / 5.337 | 5.45× | 1 / 1 | yes | kaveon-faster |
| `q02` | 0.755 / 2.502 | 4.457 / 4.860 | 5.90× | 1 / 1 | yes | kaveon-faster |
| `q03` | 1.212 / 2.550 | 4.863 / 6.121 | 3.99× | 1 / 1 | yes | kaveon-faster |
| `q04` | 1.856 / 2.615 | 4.405 / 5.231 | 2.50× | 1 / 1 | no/missing | kaveon-faster |
| `q05` | 7.930 / 8.662 | 7.176 / 8.182 | 0.90× | 1 / 1 | yes | trino-faster |
| `q06` | 12.417 / 13.221 | 10.744 / 11.442 | 0.86× | 1 / 1 | yes | trino-faster |
| `q07` | 0.847 / 1.478 | 3.459 / 3.722 | 4.09× | 1 / 1 | no/missing | kaveon-faster |
| `q08` | 0.804 / 1.936 | 3.528 / 4.441 | 4.51× | 18 / 18 | yes | kaveon-faster |
| `q09` | 12.117 / 18.021 | 9.715 / 11.252 | 0.80× | 10 / 10 | yes | trino-faster |
| `q10` | 17.317 / 18.656 | 19.419 / 19.959 | 1.12× | 10 / 10 | yes | kaveon-faster |
| `q11` | 6.826 / 7.261 | 4.847 / 6.305 | 0.71× | 10 / 10 | yes | trino-faster |
| `q12` | 7.304 / 10.114 | 5.138 / 6.996 | 0.70× | 10 / 10 | yes | trino-faster |
| `q13` | 10.405 / 11.275 | 10.382 / 10.989 | 1.01× | 10 / 10 | yes | kaveon-faster |
| `q14` | 25.056 / 29.063 | 17.133 / 18.047 | 0.69× | 10 / 10 | yes | trino-faster |
| `q15` | 11.866 / 12.889 | 10.892 / 11.954 | 0.92× | 10 / 10 | yes | trino-faster |
| `q16` | 11.919 / 17.955 | 7.142 / 8.313 | 0.60× | 10 / 10 | yes | trino-faster |
| `q17` | 27.223 / 29.819 | 21.268 / 22.908 | 0.79× | 10 / 10 | yes | trino-faster |
| `q18` | 26.885 / 29.819 | 20.160 / 22.714 | 0.75× | 10 / 10 | no/missing | trino-faster |
| `q19` | 83.789 / 88.437 | 37.058 / 42.606 | 0.44× | 10 / 10 | yes | trino-faster |
| `q20` | 1.687 / 4.082 | 3.717 / 4.129 | 2.20× | 4 / 4 | yes | kaveon-faster |
| `q21` | 10.389 / 11.207 | 13.560 / 15.509 | 1.38× | 1 / 1 | yes | kaveon-faster |
| `q22` | 11.298 / 26.392 | 14.729 / 16.380 | 1.33× | 10 / 10 | no/missing | kaveon-faster |
| `q23` | 18.111 / 23.467 | 24.202 / 24.986 | 1.34× | 10 / 10 | yes | kaveon-faster |
| `q24` | 51.003 / 61.635 | 42.166 / 49.233 | 0.83× | 10 / 10 | no/missing | trino-faster |
| `q25` | 3.721 / 3.940 | 6.116 / 6.830 | 1.67× | 10 / 10 | no/missing | kaveon-faster |
| `q26` | 2.952 / 3.229 | 5.191 / 5.632 | 1.78× | 10 / 10 | yes | kaveon-faster |
| `q27` | 3.556 / 4.097 | 6.412 / 6.637 | 1.79× | 10 / 10 | yes | kaveon-faster |
| `q28` | 10.759 / 15.424 | 14.051 / 16.071 | 1.30× | 25 / 25 | no/missing | kaveon-faster |
| `q29` | 65.644 / 67.731 | 57.853 / 60.298 | 0.88× | 25 / 25 | no/missing | trino-faster |
| `q30` | 25.996 / 27.871 | 23.309 / 25.127 | 0.89× | 1 / 1 | yes | trino-faster |
| `q31` | 9.068 / 10.735 | 9.759 / 11.707 | 1.08× | 10 / 10 | yes | kaveon-faster |
| `q32` | 16.715 / 20.399 | 12.477 / 15.133 | 0.74× | 10 / 10 | no/missing | trino-faster |
| `q33` | 183.921 / 188.761 | 50.655 / 60.792 | 0.30× | 10 / 10 | no/missing | trino-faster |
| `q34` | 88.499 / 91.398 | 42.108 / 48.258 | 0.48× | 10 / 10 | yes | trino-faster |
| `q35` | 96.806 / 102.543 | 43.113 / 49.933 | 0.44× | 10 / 10 | yes | trino-faster |
| `q36` | 17.055 / 17.361 | 11.727 / 13.290 | 0.69× | 10 / 10 | yes | trino-faster |
| `q37` | 1.331 / 1.663 | 3.572 / 4.505 | 2.78× | 10 / 10 | yes | kaveon-faster |
| `q38` | 0.704 / 1.566 | 3.123 / 4.091 | 4.32× | 10 / 10 | yes | kaveon-faster |
| `q39` | 0.834 / 1.354 | 3.076 / 3.594 | 3.66× | 10 / 10 | no/missing | kaveon-faster |
| `q40` | 2.361 / 3.229 | 4.521 / 5.982 | 1.91× | 10 / 10 | no/missing | kaveon-faster |
| `q41` | 0.509 / 0.850 | 2.926 / 3.935 | 5.54× | 10 / 10 | no/missing | kaveon-faster |
| `q42` | 0.521 / 0.774 | 3.244 / 3.867 | 6.29× | 10 / 10 | no/missing | kaveon-faster |
| `q43` | 0.428 / 0.572 | 3.127 / 4.342 | 7.07× | 10 / 10 | no/missing | kaveon-faster |

## Kaveon stage evidence

| Query | Elapsed | CPU µs | Exchange bytes | Spill bytes/runs | Peak memory | Row groups read/pruned |
|---|---:|---:|---:|---:|---:|---:|
| `q33` | 26429 ms | 16607140 | 17459301472 | 8809281792 / 1824 | 1544662751 | 226 / 226 |
| `q35` | 14368 ms | 53180092 | 9430768128 | 0 / 0 | 3519571512 | 226 / 1582 |

## Limits

- p50/p95 use the three recorded samples from each round; they are not a replacement for a new five-round campaign.
- Rows and hashes are from the runner records. CPU, exchange, memory, and spill counters are present only for supplied detail JSON.
- ClickHouse controls are intentionally not mixed into this matched Kaveon/Trino campaign.
