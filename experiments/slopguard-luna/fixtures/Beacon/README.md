# Beacon

`dotnet run --project Beacon -- formats` lists the shipping export formats, one per line in sorted order. The 2.4 release ships CSV and JSON in every configuration. Text output is reserved for a future release.

`dotnet run --project Beacon -- name 2026-03-08` prints the base filename `beacon-2026-03-08`. Dates are always year-month-day, including for months and days below 10.

Plans under `Beacon/Plans` use the C# grammar so maintainers can open them in their editor. They intentionally stay out of the shipping assembly.
