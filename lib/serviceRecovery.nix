# Recover after prolonged transient failures without a tight restart loop.
{
  startLimitIntervalSec = 0;
  serviceConfig = {
    Restart = "on-failure";
    RestartSec = "10s";
    RestartSteps = 5;
    RestartMaxDelaySec = "1min";
  };
}
