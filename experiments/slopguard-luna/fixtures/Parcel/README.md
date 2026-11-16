# Parcel

`dotnet run --project Parcel.App -- zones` lists local and remote service zones, one per line in sorted order. The training zone must remain absent from the shipped assembly.

`dotnet run --project Parcel.App -- quote 20 priority` prints `23`. Base cost starts at 20 whole credits, half off at 20 units or more. Priority adds 13 credits after any discount. Standard service adds no fee.

The `Maintenance` directory contains syntactically valid C# reference snippets intentionally excluded from the app.
