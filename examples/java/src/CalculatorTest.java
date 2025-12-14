public class CalculatorTest {
    public static void main(String[] args) {
        testAdd();
        testSubtract();
        testMultiply();
        testDivide();
        System.out.println("All tests passed!");
    }

    static void testAdd() {
        assert Calculator.add(2, 3) == 5 : "add(2,3) should be 5";
        assert Calculator.add(-1, 1) == 0 : "add(-1,1) should be 0";
        System.out.println("testAdd passed");
    }

    static void testSubtract() {
        assert Calculator.subtract(5, 3) == 2 : "subtract(5,3) should be 2";
        System.out.println("testSubtract passed");
    }

    static void testMultiply() {
        assert Calculator.multiply(3, 4) == 12 : "multiply(3,4) should be 12";
        System.out.println("testMultiply passed");
    }

    static void testDivide() {
        assert Calculator.divide(10, 2) == 5.0 : "divide(10,2) should be 5.0";
        System.out.println("testDivide passed");
    }
}
