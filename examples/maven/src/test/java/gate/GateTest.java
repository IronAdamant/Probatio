package gate;

import org.junit.Test;
import static org.junit.Assert.*;

public class GateTest {
  @Test public void testShut() { assertFalse(Gate.allow(0)); }
}
