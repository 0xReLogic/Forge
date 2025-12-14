namespace Calculator;

public class Calc
{
    public static int Add(int a, int b) => a + b;
    public static int Subtract(int a, int b) => a - b;
    public static int Multiply(int a, int b) => a * b;
    public static double Divide(int a, int b)
    {
        if (b == 0) throw new DivideByZeroException();
        return (double)a / b;
    }
}

public class Program
{
    public static void Main()
    {
        Console.WriteLine($"2 + 3 = {Calc.Add(2, 3)}");
        Console.WriteLine($"5 - 2 = {Calc.Subtract(5, 2)}");
        Console.WriteLine($"4 * 3 = {Calc.Multiply(4, 3)}");
        Console.WriteLine($"10 / 2 = {Calc.Divide(10, 2)}");
    }
}
