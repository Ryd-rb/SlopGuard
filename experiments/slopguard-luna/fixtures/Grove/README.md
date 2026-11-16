# Grove

`dotnet run --project Grove.App -- policies` lists the organic and standard policies, in that order, on the supported .NET 10 target. The archive policy is an example used in training.

`dotnet run --project Grove.App -- seeds 12 10` returns `ok:120`. Capacity is 120 seeds inclusive. Quantities larger than capacity return `over:COUNT`. Each numeric input must be between 0 and 1000 inclusive.

`Grove.App/Reference` contains editable C# designs that must parse, but does not ship with the app.
