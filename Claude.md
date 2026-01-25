使用Rust开发一个ClaudeCode/Codex无限迭代程序，核心功能就是无限循环直到Ctrl+C退出。

项目名称：AutoContinue 简称 AC

使用方法：ac [cli(claude/codex/gemini/opencode/...)] [cli参数/ac参数]
示例：ac claude --resume -cp "继续迭代，不断测试" -rp "重试"
解释：ac： AutoContinue
claude：cli软件名
--resume： cli的参数
-cp： ac的参数，continue prompt，用于继续的提示词
-rp： ac的参数，retry prompt，用于重试的提示词

ac全部参数：-h --help 显示帮助信息
-v --version 显示版本信息
-cp --continue-prompt 继续的提示词
-cpf --continue-prompt-file 继续的提示词文件，-cp和-cpf不能同时使用
-rp --retry-prompt 重试的提示词
-rpf --retry-prompt-file 重试的提示词文件，-rp和-rpf不能同时使用
-st --sleep-time 等待时间，单位秒，默认为15秒

其他输入参数一律视作cli参数，将cli参数按照顺序完整传递给cli软件

实现逻辑：在ac中启动cli，实时监测cli，如果停止了，则判断是结束了还是错误，如果是错误，则在间隔时间后进行重试，如果结束，则在间隔时间后进行继续。如果在间隔时间内恢复了（用户手动恢复），则不必输入。间隔时间是用于让用户自主回复的时间，超过该时间则认定用户没有关注，自动继续。

注意：ac依旧是能够正常显示cli，并且用户还是能操作cli的，只是多了一个自动继续的功能，其他cli功能绝对不能受到影响。

实机测试流程：ac claude -cp "继续输出" -rp "重试"
输入：输出10个数字
观察是否正常无限迭代，并在中途测试手动恢复，是否正常，测试全部功能。

核心准则：不断测试不断优化，必须经过实机测试

多轮迭代与测试，反复优化，每完成一部分都使用git提交，git中身份信息：email：MoYeRanQianZhi@gmail.com；name：MoYeRanQianZhi

持续使用git管理版本，开发时遵循开源团队协作原则，每一行代码都需要详细注释，注释文档详细记录每个函数功能与预期。