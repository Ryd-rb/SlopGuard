// Source-built shared corpus. Compile twice, with and without CANDIDATE.
// This is a fixture, not a user-supplied assembly to execute.
using System;
using System.Runtime.CompilerServices;

namespace ManagedMethodParity;

public static class Operations
{
    [MethodImpl(MethodImplOptions.NoInlining)]
    public static int Offset(int x)
    {
#if CANDIDATE
        return x - 7;
#else
        return x + 7;
#endif
    }

    // Overload disambiguation must use the encoded full method signature.
    [MethodImpl(MethodImplOptions.NoInlining)]
    public static long Offset(long x) => x + 9876543210123L;

    public static int Signed() => -33;
    public static long VeryWide() => -12345678987654321L;
    // Deliberate default-System.Text.Json cases: HTML-sensitive ASCII,
    // a literal quote and slash, short-form controls, U+0001, BMP non-ASCII,
    // a supplementary scalar and U+2028. The return string changes by side.
    public static string StringOperand() =>
#if CANDIDATE
        "<>&'+`\"\\\n\r\t\b\f\u0001\u00E9\U0001F600\u2028 C";
#else
        "<>&'+`\"\\\n\r\t\b\f\u0001\u00E9\U0001F600\u2028 R";
#endif

    public static long MinimumLong() => long.MinValue;
    public static long MaximumLong() => long.MaxValue;
    public static int SmallConstants() => -1 + 0 + 1 + 8;

    public static int Field = 109;
    public static int ReadField() => Field;
    public static Type TypeToken() => typeof(Generic<int>);
    public static int InvokeMember(int x) => Offset(x);
    public static T PassGeneric<T>(T value) => GenericHelper<T>.Identity(value);
    public static int Météo() => -401;


    [MethodImpl(MethodImplOptions.NoInlining)]
    public static int Branches(int x)
    {
#if CANDIDATE
        if (x > 2) return 22;
#else
        if (x >= 2) return 22;
#endif
        return -5;
    }

    public static int Table(int x) => x switch
    {
        0 => 10, 1 => 20, 2 => 30, 3 => 40, 4 => 50,
#if CANDIDATE
        _ => -4
#else
        _ => -3
#endif
    };

    public static int CatchAndFinally(int x)
    {
        try
        {
            if (x < 0) throw new ArgumentOutOfRangeException(nameof(x));
#if CANDIDATE
            return x + 11;
#else
            return x + 12;
#endif
        }
        catch (ArgumentOutOfRangeException)
        {
            return -77;
        }
        finally
        {
            GC.KeepAlive(x);
        }
    }

    // Compiler-generated local names belong to OffsetLocal.
    public static int OffsetLocal(int x)
    {
#if CANDIDATE
        int Local(int value) => value + 13;
#else
        int Local(int value) => value + 17;
#endif
        return Local(x);
    }

    // The generated lambda method name also belongs to Lambda.
    public static int Lambda(int x)
    {
#if CANDIDATE
        Func<int, int> calc = value => value - 101;
#else
        Func<int, int> calc = value => value + 101;
#endif
        return calc(x);
    }

#if CANDIDATE
    public static int CandidateOnly() => 333;
#else
    public static int ReferenceOnly() => 444;
#endif
}

public sealed class Container
{
    public sealed class Nested
    {
        public int NestedMethod(int x)
        {
#if CANDIDATE
            return x + 98;
#else
            return x + 97;
#endif
        }
    }
}

public sealed class Generic<T>
{
    public T Identity(T value) => value;
}

public static class GenericHelper<T>
{
    public static T Identity(T value) => value;
}
